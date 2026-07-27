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
- **Transport polling across every world-less frame** — Frontend as well as Loading — so a
  load longer than the netcode timeout does not drop every peer, and so a client with no level
  installed can be admitted at all.
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
  every peer, a connected client included. `context/research/coop-session-lobby.md` §6 records that it must
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
  through in `context/research/coop-content-compatibility.md`.
- **Replicating world gravity, and the rest of the Tier 3 item 5 redirect.** This spec
  concludes that reaction-set world gravity should be absorbed by server authority rather than
  gated by a digest, and says so in Decisions. Implementing it means widening what the host
  replicates, which has no dependency on this spec's gate and no reason to ride along with it.
  Named as the correct fix so a later reader does not reach for a reaction-lane hash instead.
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
  `context/research/coop-session-lobby.md` §6). This spec opens that ledger — see Decisions.
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
  suggests, including `entities`, `store_declarations`, `maps`, `reactions`, `crossings`,
  `trigger_events`, `trigger_pools`, `events`, the `render` profile, `ui_trees`, `theme`, and
  `frontend`. An
  earlier draft of this spec cited *theme and fonts* as the non-re-committed pair; theme is
  re-committed, through `commit_mod_ui_theme`.) Mod identity joins an existing minority of
  one rather than becoming the first exception. Worth a comment at the commit site as a second
  instance, not as a warning about a unique one. The mod **digest** does *not* diverge: it
  re-hashes on every staged commit, because a staged reload re-commits the entity registrations
  it reads, and freezing it would leave a live connection gated on a value that no longer
  describes the content the host is running.
- **Divergence 3, named — and it is this spec's own headline decision.**
  `context/research/coop-session-lobby.md` §4 as written before this spec read: "the manifest declares an id
  and a version; the client sends them at admission; the host compares. This catches honest
  drift (wrong mod, stale version), which is the actual failure mode among friends." That is
  the position the content-digest decision overturns, and the reasoning for overturning it is
  in `context/research/coop-content-compatibility.md`: a declared version does not track the breaking
  surface in either direction. Recorded as a divergence because the two documents that now
  agree with this spec — `context/research/coop-session-lobby.md` §4 and the roadmap's Phase 3.75 sub-bullet —
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
  (`context/research/coop-content-compatibility.md` Tier 3), so deferring the mod digest leaves a hole of
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
  reasoning is in `context/research/coop-content-compatibility.md`: what can break a co-op session
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
- **The mod compatibility digest covers a recursive type closure plus three mod-global lanes,
  never a lane wholesale.** Per registered entity type: the `canonical_name` a client
  materializes by, and the `PlayerMovementDescriptor` it predicts with — *including every
  tuning struct beneath `movement`*, since that is where the predicted values actually live.
  Plus `global_trigger_events`, `global_trigger_pools`, and `global_crossings`. Nothing else.
  Crossings earn their place on their own terms: both peers evaluate them over the same
  replicated slots, so differing thresholds dispatch different events off identical state.
  The full per-field disposition
  is in Task 6, and it has **three** categories rather than two — hashed, skipped because
  presentation, skipped because host-authoritative. That third category is the correction: an
  earlier draft said "hash the `entities` lane exhaustively," which would have hashed
  `behavior`, `health`, `weapon`, and `default_weapon`. Those are Tier 2 in
  `context/research/coop-content-compatibility.md` — host-owned, safe to change freely — so the rule
  as written would have demoted every peer on an enemy retune. That is the exact false
  refusal content-derived compatibility exists to prevent, arrived at from the other
  direction.
- **State-slot parity is not this digest's job.** `ReplicatedSlotSchema` already hashes every
  replicated slot's name, type, range, and scope under its own stream version, and both peers
  already compare it. A second mechanism over the same data is duplication, and that
  duplication — plus `SlotRecord`'s and `StoreDeclarationSet`'s private fields — is the whole
  reason. An earlier draft also claimed this domain avoided the IR enum; it does not, since
  `DashParams` reaches the same `IrNode` (15 variants, not the 30 that draft asserted). That
  argument is struck and the decision stands on duplication alone. The shipped fingerprint
  owns this; Task 7 fixes its one real defect, which is that it is process-cached with no
  reset and goes stale across a staged reload.
- **The IR walker is built as a general capability, and that is a deliberate over-build.**
  `NumberOrIr`/`BoolOrIr` appear only under `dash` today, but the IR module names movement as
  "the first adopter," `E18--ir-valued-reactions` has shipped, and `E10--enemy-stagger` is a
  planned adopter currently deferred on `CombatScope`: it lists IR-authored stagger tuning as
  out of scope, shipping its threshold and cooldown as plain descriptor scalars, upgradeable
  additively to `NumberOrIr` per the dash precedent once the combat `BindingScope` (Epic 16's
  `CombatScope`) lands. "IR is dash-only" describes adoption progress, not the type
  system. A dash-shaped recipe would rebuild this spec's own fail-open the first time a
  movement field becomes IR-valued, so the walker is written over `IrNode`/`IrValue` once and
  reused. It pays for itself inside this spec: `CrossingCondition::Ir` is what makes
  `global_crossings` cheap enough to cover. The IR is hashed **structurally, never by
  serializing** — serializing would auto-cover new variants and thereby destroy the compile
  error that is the entire enforcement mechanism.
- **Within every reached type the digest is a denylist, not an allowlist.** Bind every field by
  exhaustive destructuring and name the specific skips — not the reverse. An allowlist ("hash
  the fields a client simulates against") is the exact mechanism that produced the
  static-collision fail-open this spec exists to fix: a field added later by someone who never
  reads the recipe defaults to *unhashed*, and no test catches a field you forgot. A denylist
  makes the same omission fail loud instead of silent. Scope the claim honestly, though: it is
  a guarantee about fields inside the types the recipe **reaches** — which is the recursive
  closure, not the two types it names at the top — and not about manifest lanes. A new **lane**
  still escapes it, which is why the uncovered set below is named rather than assumed empty.
  The two halves of this rule are not equally new. The enum-match-with-no-wildcard half has
  repo precedent — `compute_fingerprint` over `SlotType` in
  `crates/postretro/src/netcode/state_slots.rs`, and the same pattern in `crates/net/src/wire.rs`
  and `crates/postretro/src/netcode/movement_state.rs`, each commented that a new variant is a
  compile error there. The exhaustive-struct-destructure half is new here: neither
  `kinematic_static_fingerprint` nor `compute_fingerprint` destructures exhaustively today; both
  read fields by plain access. `context/plans/done/E16--source-id-ledger/index.md` records the
  churn cost of that pattern — "Adding a field … requires updating every exhaustive struct
  literal and destructuring pattern across the crate" — and it applies in full against the
  eleven types this recipe reaches.
- **Two lanes remain knowingly uncovered — down from five — and the reason changed.**
  `reactions` and `events` are not hashed. `trigger_events`, `trigger_pools`, and `crossings`
  now are, at mod-global scope. The earlier reason for deferring all five was "its own
  IR-encoding question"; Task 6 answers that question, which is what let three of them in. What
  keeps the other two out is structural, not effort: `SequenceTarget::Entity(EntityId)` carries
  a runtime `u32` allocation handle rather than content, and — decisively — prediction-relevance
  is keyed by `PrimitiveDescriptor::primitive`, an **open string namespace**. Every guarantee
  here rests on exhaustive destructuring producing a compile error, and no compile error exists
  for "someone added a new prediction-relevant primitive." Both ways around it are mechanisms
  this spec has already rejected: hashing wholesale is the Tier 2 false refusal, hashing a
  primitive allowlist is the fail-open. Consequence, stated rather than discovered later: two
  mods whose reaction lanes differ can pass both gates and diverge on locally-simulated state.
- **Level-local reactions and crossings are covered by neither digest, and that is a third
  gap.** `DataRegistry` separates the mod-global lanes from the per-level ones `setupLevel()`
  populates. The mod digest reads the globals; the level content digest hashes `.prl` geometry
  and mover data. Script-declared per-level content sits between the two schedules. Named, not
  closed — closing it means deciding whether the level digest can depend on script execution,
  which is a boot-ordering question this spec has no reason to open.
- **Tier 3 item 5 should be replicated, not hashed — and that fix is out of scope here.**
  World gravity set from a reaction feeds local prediction on both peers, and the instinct is
  to hash the lane that sets it. The better answer is server authority: replicate the value and
  the peers agree, where a digest only lets them refuse each other when they disagree. That is
  the Tier 2 mechanism `context/research/coop-content-compatibility.md` already describes, applied to a
  case currently filed under Tier 3. This spec names the hazard and the correct redirect; it
  does not implement it, because widening what the host replicates is a replication-surface
  change with no dependency on the gate this spec builds.
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
  to participation — the one real cost of preferring hold to close, including the hazard of a
  peer that keeps its keepalive alive but never sends a matching parity message, holding the
  slot with no bound renet's own timeout reaches. Accepted, because the
  case it covers is a peer on the *right* mod whose content diverged, which is the
  recoverable case by construction. A genuinely wrong mod still closes, on the id, at admission.
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
- **The transport is polled during every world-less frame — Frontend and Loading — for
  transport advance and keepalive only.** No snapshot apply, no game logic — there is no world
  in either state. Frontend coverage is not optional: it is what makes the headline case (a
  client admitted before any level installs) reachable at all. Without it the netcode timeout,
  not the design, bounds how long a level may take to install.
- **A reject sends its reason before disconnecting.** The slot closes immediately so no
  further traffic is honored; only the socket teardown defers one poll, letting the
  reliable message flush. Without it a player on the wrong mod cannot distinguish a
  version mismatch from an unreachable host, which is most of what mod matching is for.
- **Divergence reasons become a typed enum over two closing causes and three holding
  ones** — protocol and mod id close; mod digest, level identity, and level digest hold —
  each carrying expected and received. The type is one enum rather than two so a single
  `Display` serves every diagnostic, but the closing and holding sets are distinguishable at
  the type level rather than by convention, so a later cause cannot be added to the wrong
  lane by omission. `RejectReason` is **deleted**, not wrapped: `HandshakeOutcome::Rejected`
  carries a `ClosingCause` directly, and the `Display` and `std::error::Error` impls move to
  `DivergenceReason`. `HandshakeOutcome` loses `Copy`; it already derives `Clone`. Call sites
  update in the same pass.
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
      This must hold for a field nested two levels down (`SpeedParams::run`), not only for a
      top-level one, or the criterion passes against a non-recursive recipe.
- [ ] Two mods differing only in a `DashParams` field produce different mod digests in **both**
      forms: as differing `Literal`s, and as structurally different `Ir` expressions that a
      serializer-free structural walk must distinguish. Two structurally equal `IrNode` trees
      hash equal.
- [ ] Two mods differing only in a mod-global **crossing** — a threshold, an edge, or an IR
      predicate — produce different mod digests; so do two differing only in a mod-global
      trigger event or trigger pool.
- [ ] Two mods differing only in `health`, `weapon`, `default_weapon`, or `behavior`
      produce the **same** mod digest and interoperate. So do two differing only in `light`,
      `emitter`, `mesh`, `view_feel`, or any presentation lane. So do two differing only in a
      `reactions` or `events` entry — those lanes are uncovered by decision, and a test pins
      that as intended rather than leaving it to look like an oversight.
- [ ] Declaring the same entity types in a different source order produces the same mod
      digest, and a descriptor with no `canonical_name` does not affect it at all. The same
      holds independently for each of the three mod-global lanes.
- [ ] The same content hashes to the same mod digest **in two separate processes**, including
      a descriptor whose movement tuning carries non-trivial `f32` values and an `Ir`
      expression tree — the two determinism hazards actually reachable in the domain.
- [ ] Adding a field to `EntityTypeDescriptor`, `PlayerMovementDescriptor`, **any struct
      beneath `movement`**, or any of the six lane descriptor types — `ScopedCrossing`,
      `CrossingDescriptor`, `CrossingCondition`, `TriggerEventDescriptor`,
      `TriggerPoolDescriptor`, `TriggerPoolArm` — without touching the
      digest recipe **fails to compile**; so does adding an `IrNode` variant. The recipe
      destructures exhaustively and matches without wildcards, so neither can default to
      unhashed. Verified by the pattern being present at review, not by a runtime test.
- [ ] A client whose level fails parity **keeps its connection**, receives a content
      diagnostic naming the host's map identity, and re-participates at the host's next
      matching install without reconnecting; a same-identity **level-content-digest** mismatch
      is reported as a content divergence rather than an identity one.
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
- [ ] **Debug-build criterion.** A staged hot reload that changes the mod **id or version**
      warns and leaves the installed value unchanged; no slot changes state.
- [ ] **Debug-build criterion.** A staged hot reload that changes a hashed entity field
      **recomputes and reinstalls** the
      mod digest; participating slots whose declared digest no longer matches drop to admitted
      with a mod-digest diagnostic, none of them is closed, and each demoted slot's pawn,
      replication, ownership, command, state-slot, and combat state are cleared exactly as a
      level-change demotion clears them.
- [ ] **Debug-build criterion.** A staged hot reload that changes only presentation or
      host-authoritative fields moves no
      digest and demotes nobody.
- [ ] After a host level change, no host-side level-scoped table retains an entry keyed by an
      id the unload invalidated, and the replicated-slot schema is rebuilt rather than served
      from the process cache.
- [ ] **Debug-build criterion.** A staged reload that adds or removes a store namespace
      rebuilds the replicated-slot
      schema on **both** host and client rather than serving either process cache — so the two
      peers never compare a schema fingerprint derived from declarations neither is running.
- [ ] A client demoted while participating despawns its mapped remote entities and disarms
      prediction, on both demotion triggers — level change and mod digest — rather than
      retaining a tableau that no longer updates.
- [ ] A redundant relevel naming the level already active or already in flight does not
      restart the load.
- [ ] A client joining a host that **already has a catalogued level installed** receives a
      relevel for the current map on admission, without waiting for the next transition.
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
subject. No behavior change. Verified by the relocated gate unit tests passing unchanged; no
acceptance criterion covers this task, by design — it is a move, and every criterion below
describes behavior the move preserves.

### Task 2: Mod identity in the manifest

Add a required stable id and a required version to the mod manifest. Declare both on the
`ModManifest` type at its **registration site** — the `registry.register_type("ModManifest")`
call in `crates/postretro/src/scripting/primitives/manifest.rs::register_sdk_type`, which is
where the doc strings live. `sdk/types/postretro.d.ts` and the Luau typedef are **generated
artifacts**; both carry a "do not edit by hand" banner and
`committed_sdk_types_match_current_registry` fails CI on a hand edit. Regenerate them with
`gen-script-types`, along with the `expected.d.ts` / `expected.d.luau` fixtures under
`crates/postretro/src/scripting/typedef/tests/fixtures/`. **But `defineMod` itself is not
generated.** Its declaration and doc comment are hand-written templates —
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` and `sdk_lib.luau` — copied verbatim
by regeneration, and they currently tell authors that `config.name` is the only required
field. Edit both, or the shipped SDK keeps documenting `id` and `version` as optional after
they become mandatory, and no CI guard catches it. Note the parity guard in the same
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
`StagedManifest`** (`crates/scripting-core/src/staged_manifest/transfer.rs`) — a fifth edit
site, and the one the staged path actually hands to the main thread. Without it the main-thread
commit cannot see the staged values and the first-wins warning has nothing to compare. Commit
them with the rest of the manifest at mod init, but make the commit **first-wins across
reloads**: a staged manifest whose id or version differs from the installed one logs a warning
and leaves the installed value alone. This applies to **identity only** — the mod
compatibility digest Task 7 installs follows the opposite rule and is re-hashed on every
staged commit, because a staged reload re-commits the entity registrations it reads and it
sits in the recoverable parity lane rather than the terminal admission one. Comment both rules at the commit site
together, so the asymmetry reads as deliberate: identity is frozen because admission has no
recovery path, the digest is refreshed because parity does. The identity freeze diverges from
the atomic-replace discipline most manifest lanes follow — note at the same site that
`fonts` is already non-re-committed, so mod identity joins an existing minority of one rather
than becoming a unique exception. Update the one shipped manifest under `content/` —
`content/dev/start-script.ts` — to declare both fields; note its `maps` field imports
`mapCatalog` from `content/dev/scripts/frontend-menu` rather than inlining `defineMapCatalog`
the way the Script syntax examples block shows.

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
the client→server envelope, `ClientControlMessage`, carrying admission and parity, and the
server→client envelope, `ServerControlMessage` (the name Task 4 already uses), carrying
the divergence diagnostic — as **appended** variants in the same style, and replace the
untagged decode with a match. This task defines both envelopes and the divergence variant
only; Task 4 appends the relevel variant to the server→client one. Say so at the definition
site so the two tasks do not both claim it. The messages are bitcode wire types, so they are
defined in `wire.rs` beside `ClientMessage`/`ServerMessage` and *compared* in Task 1's
`handshake.rs`; `ProtocolVersion` stays in `wire.rs` too. `DivergenceReason`, `ClosingCause`,
and `HoldingCause` are both wire types — they ride inside the server→client envelope — and the
gate's comparison output, so Task 1's split rule ("what moves is the comparison surface, not
the wire surface") does not decide their module by itself. All three are defined in `wire.rs`
with the other bitcode types and re-exported through `handshake.rs`; when a type is both, the
wire side decides its module. It **drops
`kinematic_static_fingerprint`**, keeping only `app_protocol_id` and `wire_version`, and
becomes the admission variant's constants payload — the fingerprint moves to the parity
message, so leaving the field in place would put a mutable value back in the admission lane
by accident. Note that the admission message carries `String` fields, so it cannot be `Copy`
and its constructor cannot stay a `const fn` the way `protocol_version` is today.

**Slots must retain what they declared.** `SlotTable` is `HashMap<ClientId, SlotState>` —
state only, no payload — and `process_control_messages` drops `received` after comparing. The
shipped fingerprint setter sidesteps this by closing *every* client unconditionally rather
than comparing per-slot, which is exactly what this spec replaces. Per-slot demotion needs
each slot's last-declared parity triple retained. **Use a parallel map** —
`HashMap<ClientId, ParityDeclaration>` on `NetServer` beside `pending_lifecycle`, cleared on
close — rather than widening the slot record. `SlotState` therefore **keeps `Copy`**; it gains
a variant and nothing else. (This is not the "second waiting mechanism" the gate design
refuses: it retains a received value, it does not decide anything.)

**Delete the already-accepted skip.** `process_control_messages` does not return early for a
client where `self.slots.is_accepted(client_id)` holds — the check is a `continue` inside the
`while let Some(bytes) = self.server.receive_message(client_id, Channel::Control)` loop, under
a comment reading "A client already accepted may send later control traffic," so each message
is received and discarded per-message rather than the client being skipped wholesale. Under
this design a `Participating` client re-arms and re-sends parity on **every** level install, so
that `continue` swallows precisely the message the whole spec depends on. Drain and evaluate
Control for slots in `Pending`, `Admitted`, **and** `Participating`: an admission message from
an already-admitted slot is ignored (admission is once-only per connection), a parity message
is re-evaluated on every arrival. The genuine function-level early return is separate and stays
as-is: the `let Some(fingerprint) = self.kinematic_static_fingerprint else { return outcomes; }`
guard, which is what this spec's "extend the shipped early-return" rule elsewhere refers to.

The version field is carried for diagnostics and **must not gate** — the only comparison
permitted is the one that emits a host-side `info` log naming both versions when an admitted
client's version differs from the installed one. Comment it at the comparison site so a later
reader neither "fixes" the missing gate nor deletes the log.

**The divergence reason is a two-level enum**, not a flat one:
`DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}`, with `Display` on the
outer so one impl serves every diagnostic. A flat five-variant enum would be distinguishable
only by matching on it — which is the convention this is meant to replace, so the nesting is
the point rather than decoration. `ClosingCause` covers protocol and mod id; `HoldingCause`
covers mod digest, level identity, and level digest. `crates/net/src/slots.rs` already defines
`CloseCause { Disconnect, Timeout }`, used as `SlotState::Closed { cause: CloseCause }` and
`SlotEvent::Closed { cause: CloseCause }`; `ClosingCause` is a distinct type and both coexist in
the crate. `CloseCause` says how a connection ended; `ClosingCause` says why admission refused
it. Each carries expected and received, with
the mod-id cause quoting both peers' declared versions and the level causes distinguishing an
identity mismatch from a same-identity digest mismatch. Each cause's payload is pinned:

| Cause | Lane | Payload |
|---|---|---|
| `Protocol` | closing | `expected: ProtocolVersion`, `received: ProtocolVersion` (two `u32`s each, post-drop) |
| `ModId` | closing | `expected: String`, `received: String`, plus `expected_version: String`, `received_version: String` for the diagnostic |
| `ModDigest` | holding | `expected: [u8; 32]`, `received: [u8; 32]` |
| `LevelIdentity` | holding | `expected: String`, `received: String` |
| `LevelDigest` | holding | `identity: String`, `expected: [u8; 32]`, `received: [u8; 32]` — identity is carried so the diagnostic can say "same map, different content" |

`RejectReason` today is a **struct** with an `impl std::error::Error`. It is **deleted**:
`HandshakeOutcome::Rejected` carries a `ClosingCause` directly, and the `Display` and
`std::error::Error` impls move to `DivergenceReason`. Keeping it as an outer wrapper would
leave two spellings of the same idea, and its `ProtocolVersion`-typed fields are invalidated
by the message split anyway. `HandshakeOutcome` **loses `Copy`; it already derives `Clone`.**
The real work is two reuse-after-move sites in `process_control_messages` (the
malformed-decode and validate-failure branches each move `reason` into `reject` and then reuse
it in the outcome) plus two `Some(*reason)` derefs. `NetServer::reject`'s signature is
`fn reject(&mut self, client_id: ClientId, _reason: RejectReason)` — the parameter is unused and
underscore-prefixed today, so the current moves are free; it is Task 3's own requirement that
`reject` enqueue the typed cause on Control that makes the parameter live and turns those two
sites into real work. The derefs are, in
`loopback_diverged_app_version_is_rejected_with_typed_reason` and
`loopback_mismatched_kinematic_static_content_is_rejected_before_snapshots`.

**Slot machine.** Add an `Admitted` **variant to `SlotState`** between `Pending` and
`Accepted`, rename `Accepted` to `Participating`, and add the demotion transition
`Participating → Admitted` emitting a new lifecycle event beside the existing close event.
`SlotEvent::Accepted` is renamed to `SlotEvent::Participating` the same way, matching the
`SlotState` rename — it is not a new variant left beside a retained `Accepted`. Worth stating
because `SlotEvent::Accepted` is never emitted today; it is matched-but-ignored in
`netcode/mod.rs`, so under this spec `SlotEvent::Participating` begins being emitted for the
first time. Keep `Closed { cause }` terminal and every existing idempotence property — with two
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
to neither, so the vocabulary is fixed here rather than left for Task 7 to invent:

| Verdict | Rides | Shape |
|---|---|---|
| admission pass | `handshakes` | `HandshakeOutcome::Admitted { client_id }` |
| admission reject | `handshakes` | `HandshakeOutcome::Rejected { client_id, cause: ClosingCause }` |
| parity hold | `handshakes` | `HandshakeOutcome::ParityHeld { client_id, cause: HoldingCause }` |
| participation transition | `lifecycle` | `SlotEvent::Participating { client_id }` |
| demotion | `lifecycle` | `SlotEvent::Demoted { client_id, cause: HoldingCause }` |

`SlotEvent` derives `Copy` today in `crates/net/src/slots.rs`; `HoldingCause` carries `String`
and `[u8; 32]` payloads, so `SlotEvent::Demoted { client_id, cause: HoldingCause }` costs
`SlotEvent` its `Copy` derive — it keeps `Clone`. `host_handle_lifecycle`
(`crates/postretro/src/netcode/mod.rs`) takes `lifecycle: &[SlotEvent]`, so by-value binding
sites there break and need updating to bind by reference or clone.

`ServerPoll` keeps its two vectors — no third field. The split follows the shipped rule:
`handshakes` carries *gate verdicts about a message just evaluated*, `lifecycle` carries *slot
state transitions the engine must clean up after*. `process_control_messages` returns both.
Task 7 hangs the pawn spawn on `SlotEvent::Participating` and the cleanup on
`SlotEvent::Demoted`. Note that the
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
is Snapshot and `send_input` is Input — so name the new one `NetServer::send_control`. On an
**admission** reject —
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
`poll_handshakes` is the in-memory relay path `harness.rs` and `transport.rs`'s relay tests
(`relay_accepted_pair`, `fingerprint_change_surfaces_exactly_one_close_event`) use — the
real-UDP loopback tests drive `NetServer::update` through `run_handshake`. "The next poll"
must mean both, or relay-driven tests wedge on an undrained pending-disconnect list.

**Client side — installed values first.** `NetClient` today has exactly one installed-value
setter, `set_kinematic_static_fingerprint`. It needs the same shape as the server: an
`Option<(mod id, mod version)>` for admission and the two parity `Option`s — `Option<mod
digest>` and `Option<(level identity, level digest)>` — each with a named setter Task 7 calls.
The client compares nothing; it only declares. Its send precondition is explicit, the same
shape as the server's: the client sends parity only when both of its parity `Option`s are
present, combining them into the comparable triple only then — a partial install is not a
parity value on the client side either. Both roles' setters are named in Task 7.

**Client side — flags.** Split `handshake_sent` into an admission flag and a parity flag,
replacing today's self-disconnect. The admission flag is **not** "sent once on connect": mod
identity is not installed until mod init, which runs after `Session::build` constructs the
endpoint, so it is sent once on the first poll at which the transport is connected *and* the
identity is present — the same queue-until-installed rule the server's gate uses, applied on
the sending side. The parity flag re-arms whenever either parity source changes — level
install **or** staged mod commit. Both flags are duplicated across `NetClient::update` and
`NetClient::update_connections`, so the split lands twice, and the accessor is the
loop-termination condition in `harness.rs`'s `pump_client_to_server`.

Bump `PROTOCOL_ID` and `WIRE_VERSION`, and re-stage the existing both-gates regression test in
`transport.rs` — which hard-asserts their exact values with bump-specific failure messages —
to the new pair, with the previous pair as the refused peer. That test is what satisfies the
last acceptance criterion. Unit-test the gate and the slot machine without sockets, including
a mod-digest change demoting a participating slot with no level involved, and a demoted slot
re-participating and re-emitting its promotion.

### Task 4: Relevel message and client-follow

Give the host a way to name the next map and the client a way to follow it. **Append** a
relevel variant to the server→client Control envelope Task 3 defines — Task 3 reserves the
slot and defines no relevel; this task adds it — carrying one map catalog id,
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
engine drains. **Retype the existing `NetClient::drain_control`** from `Vec<Vec<u8>>` to
`Vec<ServerControlMessage>` — it has no callers today, so the change costs nothing — rather
than introducing a `ClientPoll` return type. A `ClientPoll` would mean changing
`NetClient::update`'s signature (`Result<(), _>` today) *and* `update_connections`'s (`()`),
touching every call site to carry a value only this task consumes. The drain must run from the
world-less frames Task 5 opens as well as from Running, since a relevel can arrive while an
earlier load is in flight. The engine enqueues
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
`crates/postretro/src/main.rs`. "Game logic" is left ambiguous if stated only as a
prohibition, since Task 4 drains relevel from these frames and calls
`App::enqueue_level_request`, and Task 7's client-side demotion despawns remotes from them. The
boundary, concretely: forbidden is snapshot apply, state-crossing detection, and the
simulation tick — there is no world outside Running, and the snapshot-apply ordering contract
(apply before state-crossing detection, within the Game-logic stage) has no meaning there.
Permitted is transport advance, handshake processing, keepalive, Task 4's relevel drain, and
Task 7's divergence-diagnostic drain. Splash is out, though not for the reason an earlier
draft gave: `Session::build` runs *inside* the final splash frame, via `install_pending_session`
in `run_splash_frame` (`crates/postretro/src/startup/splash_lifecycle.rs`), which then
transitions straight to Frontend or Loading and returns. So splash does not precede the
endpoint's construction — it contains it — but no splash frame ever holds a constructed
endpoint past that transition, so none needs to advance it.

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
function-local `const` inside the recipe, not a shared constant. **Add** the world parameter,
do not substitute it: the signature becomes
`pub(crate) fn level_content_digest(geometry: &KinematicGeometry, world: &LevelWorld) -> [u8; 32]`.
Replacing `&KinematicGeometry` would silently drop the mover list, per-mover collision, and
waypoints the fingerprint covers today; the caller in `install_level_payload` already holds
both. Rename to `level_content_digest` — it is no longer kinematic-only — and carry
the rename through its engine-side call chain, which reaches outside this file:
`NetEndpoint::set_kinematic_static_fingerprint` (`crates/postretro/src/netcode/mod.rs`) and
its caller in `install_level_payload`. Test that two levels differing **only** in static
collision produce different digests, that identical geometry with differing entity placements
produces the same one, and that two mover-less levels with different brushwork no longer
collide.

**Mod compatibility digest.** Two entity-descriptor types and their nested tuning structs,
plus three mod-global registry lanes. Not the `entities` lane wholesale, and not every lane.

*Source.* Read from `ScriptCtx::data_registry` after mod init commits, **not** from
`ModManifestResult`: `ScriptingCore::drain_manifest_registrations` does
`std::mem::take(&mut manifest.entities)`, so the manifest's own `entities` is an empty `Vec`
by the time anything could hash it. The registry is the same read `App::net_poll_and_apply`
already uses to build `net_descriptors`, so this needs no new visibility — true about
readability, not about access path: `net_poll_and_apply` runs from the Running gameplay block,
while the digest installs at mod init and at staged commit, two different call sites with
different borrow environments, and plumbing both is Task 7's to do. Read the
**mod-global** lanes specifically — `global_trigger_events`, `global_trigger_pools`,
`global_crossings` — not their per-level counterparts. `DataRegistry` holds both sets
separately, the globals populated from the manifest and the per-level ones from
`setupLevel()`; only the globals are mod content. The per-level ones are a named gap, below.

*Domain, part 1: entity descriptors.* Per entity type in the registry, hash exactly two things:

| Field of `EntityTypeDescriptor` | Disposition |
|---|---|
| `canonical_name` | **hashed** — the wire's entity class |
| `movement` (`PlayerMovementDescriptor`) | **hashed**, minus `view_feel` |
| `default_weapon`, `weapon`, `health`, `behavior` (`BehaviorGraphDescriptor`) | skipped — **host-authoritative** |
| `light`, `emitter`, `mesh` | skipped — **presentation** |

Three categories, not two. A field can be excluded because a client never sees it *or*
because the host owns it, and collapsing those into one "presentation" bucket is what an
earlier draft got wrong: `behavior`, `health`, and `weapon` are Tier 2 in
`context/research/coop-content-compatibility.md` — safe to change freely — so hashing them would
demote every peer on an enemy retune, which is the false refusal this whole policy exists to
prevent. Inside `PlayerMovementDescriptor` the nine remaining fields (`capsule`, `ground`,
`air`, `fall`, `stuck_stop_enabled`, `stuck_stop_threshold`, `dash`, `forgiveness`, `crouch`)
are all prediction inputs; `view_feel` is documented render-only and is the one skip.

**The domain is recursive, and naming only the two outer types was a defect.** Seven of those
nine fields are structs, not scalars — the tuning values a client actually predicts with live
one and two levels down (`GroundParams::accel`, `SpeedParams::run`, `AirParams::jump_velocity`).
The hashed closure is therefore `EntityTypeDescriptor`, `PlayerMovementDescriptor`, **and every
struct beneath `movement`**: `CapsuleParams`, `GroundParams`, `SpeedParams`, `AirParams`,
`FallParams`, `DashParams`, `ForgivenessParams`, `CrouchParams`. The destructure rule below
applies to each of them. Scoped to the two outer types alone, adding `AirParams::air_control`
would compile clean and default to unhashed — the exact silent omission the rule exists to
stop, one level down from where it was looking.

*The IR walker is a general sub-recipe, not a dash special case.* `DashParams` carries five
`NumberOrIr` fields (`boost_speed`, `momentum_retention`, `steer_control`, `dash_drag`,
`cooldown_ms`), one `BoolOrIr` (`preserve_vertical`), and a plain `air_dashes: u32`. Both
wrappers are `enum { Literal(..), Ir(IrNode) }` over `postretro_foundation::ir::IrNode` — 15
variants (`Const`, `Input`, `Add`, `Sub`, `Mul`, `Div`, `Clamp`, `Lerp`, `Lt`, `Le`, `Gt`,
`Ge`, `Eq`, `Ne`, `Select`), tree-recursive through `Box<IrNode>`, with
`IrValue::{Bool(bool), Number(f32)}` at the leaves. So the recipe walks an IR tree, and it
writes that walker as a reusable capability — `hash_number_or_ir`, `hash_bool_or_ir`,
`hash_ir_node`, `hash_ir_value` — rather than inlining it at dash's six fields.

The reason is adoption trajectory, not tidiness. The IR module's own doc names movement as
"**the first adopter**" of the substrate; `E18--ir-valued-reactions` has already shipped, and
`E10--enemy-stagger` is a planned adopter currently deferred on `CombatScope` — it lists
IR-authored stagger tuning as out of scope, shipping plain descriptor scalars upgradeable
additively to `NumberOrIr` once Epic 16's `CombatScope` lands, rather than drafting against the
wrappers today. The wrappers sit in the shared typedef
surface (`crates/scripting-core/src/typedef/common.rs`). "IR appears only under `dash`" is a
statement about how far adoption has got, not a property of the types. A dash-shaped recipe
would rebuild this spec's own fail-open the first time `GroundParams::accel` becomes
IR-valued — a new prediction input, silently unhashed. The general walker also pays for
itself immediately: it is what makes `global_crossings` cheap, below.

**Hash the IR structurally, not by serializing it.** `IrNode`'s serde representation is
pinned and byte-matched, which makes `serde_json::to_vec` tempting, and it would even
auto-cover new variants. Reject it: serializing produces no compile error, so a new variant
would silently change every digest instead of stopping the build — and the compile error is
the entire mechanism this recipe is built on. Walk it with an exhaustive `match`, no wildcard
arm, a discriminant byte per variant, `Box` recursion, a length-prefixed `hash_str` for
`Input { name }`, and `hash_f32` bit patterns for `IrValue::Number`.

An earlier draft of this task claimed the chosen domain *avoided* the IR enum, and offered
that as a reason to prefer it over hashing state-slot data. That was false — `dash` reaches
the same `IrNode` — and it is struck. (It also said "~30 variants"; there are 15.) The real
reason state slots stay out is duplication with a shipped fingerprint, which still holds.

Descriptors whose `canonical_name` is `None` are **excluded entirely**. A client materializes
remote entities by `entity_class` matched against `canonical_name`, so an unnamed descriptor
cannot cross the wire — which also supplies the total order the sort needs, since the
remaining set is keyed by a present name, and `DataRegistry::upsert_entity_type` already
guarantees that name is unique. The sort is therefore total without a tiebreak.

*Domain, part 2: three mod-global registry lanes.* All three are prediction-relevant, none is
presentation, and all three are cheap once the IR walker exists. Hash each lane's entries in a
key-sorted order (by `tag` for the trigger lanes; for crossings by `slot` then `fire`, with a
structural tiebreak, since `slot` is `Option` and not unique):

| Lane | Shape | Notes |
|---|---|---|
| `global_trigger_events` | `TriggerEventDescriptor { tag, event, fire, levels }` | all `String`/`Vec<String>`; already derives `Hash` |
| `global_trigger_pools` | `TriggerPoolDescriptor { tag, arm, levels }`, `TriggerPoolArm::{Count(u32), Percentage(f64)}` | needs a **`hash_f64`** helper — only `hash_f32` exists today |
| `global_crossings` | `ScopedCrossing { crossing, levels }`; `CrossingDescriptor { slot: Option<String>, condition, max: f32, edge: Option<String>, fire: Vec<String> }`; `CrossingCondition::{Below{threshold}, Above{threshold}, Ir(IrNode)}` | the `Ir` arm is why the general walker pays for itself |

Crossings are the load-bearing one. Both peers run the crossing detector over the same
replicated slot values, so two mods whose thresholds or predicates differ dispatch **different
events off identical state** — divergence with no map involved and nothing today to catch it.

*Two lanes stay uncovered, and the reason is not the one an earlier draft gave.* `reactions`
and `events` are **not** hashed. The previous reason — "its own IR-encoding question" — is
retired, because this task now answers that question. The real blockers are two. First,
`SequenceStep::id` is `SequenceTarget::{Entity(EntityId), Activators, FiredTrigger}` and
`EntityId` is a newtype over `u32`: a runtime allocation handle, not content, so hashing it
would bind the digest to spawn order. Second and decisively, whether a reaction is
prediction-relevant is keyed by `PrimitiveDescriptor::primitive` — an **open string
namespace**, not a struct shape. Every guarantee in this recipe rests on exhaustive
destructuring producing a compile error, and no compile error is reachable for "someone added
a new prediction-relevant primitive." Both escapes are mechanisms this spec has already
rejected elsewhere: hashing the lanes wholesale demotes every peer when a `playSound` or
`setEmitterRate` argument changes, which is the Tier 2 false refusal; hashing an allowlist of
primitives is the allowlist mechanism that produced the static-collision fail-open. Note that
the `serde_json::Value` payloads are *not* the obstacle — `preserve_order` is off in this
workspace, so `serde_json::Map` is a `BTreeMap` and iterates key-sorted for free.

*A third gap, newly identified and covered by neither digest.* `DataRegistry` holds per-level
`reactions`/`crossings`/`trigger_events`/`trigger_pools` from `setupLevel()` separately from
the mod-global lanes above. The mod digest reads the globals; the level content digest hashes
`.prl` geometry and mover data. **Level-local, script-declared crossings and reactions fall
between them.** The crossing and trigger halves have no blocker of their own — only no digest
that looks there, because they install on the level schedule rather than the mod one. Named
here rather than left implicit; closing it is a level-digest-schedule question this spec does
not open.

*State slots are not in this digest.* `ReplicatedSlotSchema`
(`crates/postretro/src/netcode/state_slots.rs`) already hashes every replicated slot's dotted
name, type, range, and scope under its own `FINGERPRINT_STREAM_VERSION`, and both peers
already compare it. Do not build a second mechanism over the same data. The blocker is
duplication plus the private fields a second recipe would have to reach — `SlotRecord`'s
`write_generation` and `StoreDeclarationSet`'s `BTreeMap`. It is **not** the `IrNode` that
`SlotSchema::accumulate` holds: this recipe walks that enum anyway, and an earlier draft both
offered it as a reason and miscounted it at ~30 variants. State-slot parity is that
fingerprint's job; this task's only slot-related work is the cache defect Task 7 fixes.

*Determinism rules.* Enums are hashed through a `match` with **no wildcard arm** — struct
destructuring gives no exhaustiveness over enums, so this is a separate rule and not a
consequence of the destructure rule. It binds hardest on `IrNode`'s 15 variants and on
`CrossingCondition`. `Option` writes a presence byte, and every string and sequence is
length-prefixed via `hash_str`/`hash_len`, so two distinct descriptor sets cannot concatenate
to the same stream. Floats hash as bit patterns through `hash_f32` (and a new `hash_f64` for
`TriggerPoolArm::Percentage`), never through formatting.

Map-valued fields are hashed in **key-sorted order**. State plainly what this rule is doing
here: **no map-valued field is reachable in today's domain.** An earlier draft justified the
rule with `HealthDescriptor::zone_multipliers`, a `std::collections::HashMap` with
`RandomState` — but it hangs off `EntityTypeDescriptor::health`, which the disposition table
skips as host-authoritative, so the recipe never sees it. The example outlived the domain
change that excluded it. The rule stays as a forward-looking guard, satisfied in advance by
the `BTreeMap`-backed JSON payloads should the reaction lanes ever come in, and it is not
what makes today's digest cross-process stable. What does: `f32` bit patterns, the
`IrNode` walk, and the lane sort orders above.

*Enforcement.* Within **every** type the recipe reaches — the two entity types, the eight
structs beneath `movement`, the six lane descriptor types (`ScopedCrossing`,
`CrossingDescriptor`, `CrossingCondition`, `TriggerEventDescriptor`, `TriggerPoolDescriptor`,
`TriggerPoolArm`), and `IrNode`/`IrValue` — bind
every field by exhaustive destructuring. `let Descriptor { a, b, .. }` is forbidden, no rest
pattern; enums match with no wildcard arm. Route skips through two labelled blocks,
`// not hashed: host-authoritative` and `// not hashed: presentation`. Adding a field then
fails to compile until someone classifies it, and that compile error is the mechanism. The
no-wildcard-arm half has repo precedent — `compute_fingerprint` over `SlotType` in
`crates/postretro/src/netcode/state_slots.rs`, and the same pattern in `crates/net/src/wire.rs`
and `crates/postretro/src/netcode/movement_state.rs`. The exhaustive-struct-destructure half is
new: neither `kinematic_static_fingerprint` nor `compute_fingerprint` destructures exhaustively
today, both read fields by plain access, and `context/plans/done/E16--source-id-ledger/index.md`
records the churn cost of the pattern this task adopts. Scope
the claim honestly: it is a guarantee about fields inside the **reached** types, not about
manifest lanes. A new *lane* still escapes it, which is why the uncovered set above is named
rather than assumed empty. Two earlier drafts got this scope wrong in opposite directions —
one applied it at lane granularity, which would have hashed host-authoritative fields; the
next scoped it to two named types, which left the values it protects one level below the
guarantee.

*Placement, signature, and shared helpers.* Put the recipe engine-side as a sibling of the
level recipe — `crates/postretro/src/mod_digest.rs` — not in `scripting-core`: the manifest
lane stays unaware of netcode, exactly as the mover recipe keeps its byte layout out of the
net crate. It is a pure function over borrowed slices, so Task 7 owns every registry access:

```rust
pub(crate) fn mod_compatibility_digest(
    entities: &[EntityTypeDescriptor],
    trigger_events: &[TriggerEventDescriptor],
    trigger_pools: &[TriggerPoolDescriptor],
    crossings: &[ScopedCrossing],
) -> [u8; 32]
```

`hash_len`, `hash_str`, `hash_vec3`, and `hash_f32` are **private free functions inside
`runtime_movers.rs`** today, so a sibling module cannot call them. Move all four, plus the new
`hash_f64` and the IR walker, into a shared `pub(crate)` module — `crates/postretro/src/content_hash.rs` —
that both recipes import. Add `hash_u32` to the moved set as well: the moved helpers have no integer entry, but
`LevelWorld::indices` and `TriggerPoolArm::Count` are `u32` (the IR discriminant is a `u8`), and
the existing recipe hashes indices inline via `hasher.update(&index.to_le_bytes())` —
without `hash_u32` the shared module ends up with two spellings of integer hashing. That move is part of this task, not an incidental refactor: without
it the two recipes either duplicate the helpers or diverge.

*Epoch.* A function-local `const MOD_DIGEST_EPOCH: u32 = 1`, mirroring `FINGERPRINT_EPOCH`'s
shape and bumped whenever the recipe changes.

Test: order-insensitivity across all four inputs; that a `movement` edit moves the digest,
including one nested two levels down (`SpeedParams::run`) and one behind an IR wrapper
(`DashParams::boost_speed` as both a `Literal` and an `Ir`); that two structurally different
`IrNode` trees hash differently and two equal ones hash the same; that a `behavior`, `health`, or
`weapon` edit does **not** move it; that a presentation edit does not; that a `view_feel` edit
does not; that two mods differing only in a `reactions` or `events` entry produce the **same**
digest, pinning AC 13's uncovered-lanes claim rather than leaving it to look like an oversight;
that a crossing threshold or predicate edit **does**; that a trigger-event or
trigger-pool edit does; and that the same content hashes identically **in two separate
processes** — the real hazards being `f32` bit patterns and the `IrNode` walk, not map order.

### Task 7: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity:** after mod init commits, install
the id and version on the net endpoint through a setter mirroring the fingerprint's, reached
through the same `session.net_endpoint` borrow `install_level_payload` uses; single-player
has no endpoint and skips. Install once, first-commit-wins, and on a staged reload that
changes either value log the warning Task 2 specifies.

Both roles need setters, not just the host: `NetEndpoint` installs identity and both parity
sources on `NetServer` (which compares) and on `NetClient` (which only declares), so every
setter named in this task lands twice. Single-player has no endpoint and skips.

**Mod digest:** compute Task 6's recipe over `ScriptCtx::data_registry` — passing `entities`,
`global_trigger_events`, `global_trigger_pools`, and `global_crossings` as the four slices its
signature takes — and install it as the parity lane's first source — at mod init **and again after every staged commit**, since a
staged reload re-commits the entity registrations the recipe reads. The staged seam is
`App::poll_staged_manifest_results`
(`crates/postretro/src/startup/staged_manifest_lifecycle.rs`), which the spec previously left
unnamed. The entire staged-manifest mechanism is debug-build-only, and nothing said so until
now: `ScriptRuntime::poll_staged_manifest_builds` returns an empty `Vec` in release, and
`ScriptRuntime::commit_staged_manifest_result` wraps its body in `#[cfg(debug_assertions)]` and
otherwise returns a fourth outcome variant, `StagedManifestCommitOutcome::ReleaseNoop` (both in
`crates/scripting-core/src/runtime/core.rs`). Four things the seam forces: install only on
`StagedManifestCommitOutcome::Committed`; the match at the seam must cover all four outcome
variants, including `ReleaseNoop`; place the install inside a `self.session` borrow scope
compatible with the one that function
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
now proves. **Demotion:** route `SlotEvent::Demoted` into `host_handle_lifecycle` so it
runs the same per-slot cleanup a close runs — do not duplicate that cleanup. **Host unload
reset:** `reset_level_scoped_client_state` early-returns for the host role today; give it a
host arm that clears the level-scoped host tables (movement owners, slot pawns, replicable
set, weapon owners, open shots, command queues) whose entries the unload has invalidated.
These are **two different cleanups** and the spec means both: the per-slot close cleanup keyed
by `ClientId` that a demotion reuses, and the level-scoped host table reset keyed by level
lifetime. Neither subsumes the other — a mod-digest demotion runs the first with no level
change, a level unload runs the second for tables no live slot owns.

**Client-side demotion.** A client that receives a `Holding` diagnostic while participating
must react, and the two demotion triggers differ: a level-change demotion clears the client's
world by unloading it, while a mod-digest demotion leaves the world loaded. The seam is the
`NetworkId → EntityId` map and prediction arming. Default behavior, pending the open question
below: despawn mapped remotes and disarm prediction on **any** demotion, so the two triggers
converge on one client-side state rather than leaving a frozen tableau on one path. Named here
so resolving that question is an edit to this paragraph rather than a new task.

**Replicated-slot schema cache — work, not a check.** An earlier draft said "confirm the host
state-replication schema is rebuilt per level rather than cached for the process." It is not:
`HostStateReplication::schema` is built lazily through `get_or_insert_with` and has no reset
path, and the client-side `schema`/`net_schema` cache identically. So give all of them a reset
on level unload **and** on a staged commit that changes store declarations. This matters
beyond tidiness now that state-slot parity is owned by `ReplicatedSlotSchema` rather than by
the mod digest: a staged reload that adds a namespace otherwise leaves both peers comparing a
fingerprint derived from schema neither is still running.

**Write the session-state ledger down.** The roadmap requires session-surviving state be
enumerated rather than accreted, and this spec claims to open that ledger three times without
anything recording it. Add the list to `context/lib/networking.md` in the same pass: one entry
— the connection, comprising its `ClientId`, its `SlotState`, and its retained parity
declaration — plus the subtraction rule that defines the rest (everything level-scoped and
everything per-slot clears on demotion, so what survives is exactly what a demotion does not
touch). Spec 2 adds the seat and roster to a named list rather than discovering one.

Keep the `main.rs` edit to redirecting the two triggers, plus the deferred
`accepted_clients`/`is_accepted` mechanical rename pass Sequencing assigns this task —
splitting that file is out of scope and explicitly deferred by `runtime-level-lifecycle`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split every net-crate edit lands in.

**Phase 2 (concurrent):** Task 2, Task 3, Task 5, Task 6. Disjoint in their primary files —
scripting-core, the net crate, the world-less frames, the two hash recipes — with two crossings
named rather than assumed away:

- **Task 3 is not net-crate-local.** Renaming `Accepted` to `Participating` escapes the crate:
  `NetServer::accepted_clients` is called from `crates/postretro/src/main.rs` and
  `crates/postretro/src/netcode/mod.rs`, `NetServer::is_accepted` from
  `trigger_state_channel_harness_test.rs` and, in-crate, twice from `crates/net/src/harness.rs`
  — needing no deprecated alias there, but touched by both Task 1's relocation and Task 3's
  rename — and the exhaustive `SlotEvent::Accepted` arm in
  `netcode/mod.rs` breaks on the new demotion variant by design. Task 3 therefore **keeps
  `#[deprecated]` aliases** for `accepted_clients`/`is_accepted` and defers the mechanical
  rename to Task 7, rather than carrying a rename pass over `crates/postretro` in Phase 2.
  Without one of the two the workspace does not compile at the end of Phase 2; the alias branch
  is chosen because it also resolves two of the three file collisions below.
- **Task 6 has one existing caller.** `kinematic_static_fingerprint` is called today from
  `install_level_payload`, and Task 6 both renames it and changes its parameter, so Task 6
  updates that call site in place. Task 7 later rewrites the surrounding lines. This is a
  two-line overlap, not a conflict, but it is not "no caller until Phase 4" as an earlier
  draft claimed.
- **Four same-file collisions inside Phase 2, resolved by rule rather than by hope.** Task 5
  and Task 6 both edit `crates/postretro/src/startup/lifecycle.rs` — Task 5 the loading frame,
  Task 6 the `install_level_payload` call site — in disjoint functions, so they merge. Task 5
  and Task 3's rename pass both edit `crates/postretro/src/main.rs`, and Task 3's rename pass
  and Task 6's rename both edit `crates/postretro/src/netcode/mod.rs`. Resolve those two by
  **taking the deprecated-alias branch** of the choice above: Task 3 keeps
  `accepted_clients`/`is_accepted` as `#[deprecated]` aliases and does not touch `main.rs` or
  `netcode/mod.rs` bodies, leaving the mechanical rename to Task 7 in Phase 4. The one
  unavoidable break stays: the exhaustive `SlotEvent` arm in `netcode/mod.rs` must gain the
  demotion variant in Task 3, which is a one-arm edit rather than a file-wide pass. The fourth:
  Task 6 creates `crates/postretro/src/mod_digest.rs` and `crates/postretro/src/content_hash.rs`,
  whose `mod` declarations land in `main.rs` — the same binary crate root Task 5 also edits.
  Two lines, in a different region of the file from Task 5's loading-frame edit, so they merge
  the same way the others do.

**Phase 3 (sequential):** Task 4 — consumes Task 3's slot states and Control envelope.
**Phase 4 (sequential):** Task 7 — consumes the setters from Task 3, the committed identity
from Task 2, both recipes from Task 6, and the client-follow drain from Task 4.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC 1, 3, 4, 18 |
| **Admission carries only values that cannot change for a live connection** | Task 3 (lane assignment), Task 2 (identity frozen at first commit) | The lane is chosen by convenience, not by mutability — a future compared value put in admission "because it is known early" becomes an unrecoverable close the moment it can be reinstalled. The mod digest was exactly that mistake, caught on review | AC 4, 24 |
| Content parity is proven for the *current* content, not the joining one | Task 3 (demotion on source replacement), Task 7 (per-install level pair, per-commit mod digest) | A demotion that failed to clear state would leave stale ids addressable; a parity value that stopped being reinstalled would gate on history | AC 18, 19, 24 |
| A demotion clears exactly what a close clears, **host side** | Task 3 (event), Task 7 (routed into the existing cleanup) | Shared with the close path — a second cleanup implementation would drift. Both demotion triggers, level and mod digest, run the one path on the host, where a named close cleanup exists to be "exactly" equal to. The client side has no such cleanup to reuse — Task 7 defines the client-side demotion behavior (despawn mapped remotes, disarm prediction) rather than reusing an existing close path | AC 18, 24, 28 |
| Level identity discriminates two distinct levels within one mod, and distinguishes addressing modes for the same file — not two distinct levels across mods, which a catalog id is mod-scoped and cannot discriminate; that cross-mod collision is closed by admission, not by identity | Task 7 (catalog id, normalized path fallback) | The per-level gate is a no-op if two levels within a mod can collide, and the addressing-mode case is the one that collides in practice | AC 9 |
| The level digest discriminates content the identity cannot | Task 6 (static collision folded in, epoch 2) | Two maps sharing an identity but differing in brushwork must not compare equal — this is the fail-open the spec exists to close, and it is only tested if identity is held constant | AC 8 |
| Every input a client simulates against is either covered by a digest **or named as uncovered** | Task 6 (both recipes, three mod-global lanes), Decisions (`reactions`/`events`, and level-local script content) | A new client-local simulation input added later is silently ungated unless its recipe is widened too — static collision was exactly that omission. The weaker "or named" form is deliberate: two lanes and the level-local set are uncovered, and a total-coverage claim would be false | AC 8, 10, 11, 12, 13, 16 |
| **The digest reaches every value it protects, not just the types it names** | Task 6 (recursive closure through `movement`'s eight sub-structs; the general `IrNode`/`IrValue` walker) | A recipe scoped to the outer types leaves the predicted values one and two levels below the guarantee — and an IR-valued field walked dash-shaped fails open the moment a second field adopts the substrate | AC 10, 11, 16 |
| The mod digest describes the content the host is running now | Task 7 (re-hash on every staged commit) | Freezing it — the first draft's rule — gates live connections on a value the reload already replaced, silently, in the builds where co-op is developed | AC 24, 25 |
| The mod digest is stable across processes | Task 6 (`f32` bit patterns, structural `IrNode` walk, per-lane sort orders) | The reachable hazards are float formatting and IR traversal order, not map iteration — no map-valued field is in the domain, and anchoring this to one that is not reachable is how the criterion came to pass vacuously | AC 15 |
| Host-authoritative fields never affect compatibility | Task 6 (three-category disposition table) | Hashing `behavior`, `health`, or `weapon` demotes peers on an enemy retune — the false refusal the whole content-derived policy exists to prevent | AC 13 |
| Mod version is carried and never compared | Task 2 (SDK docs), Task 3 (commented at the comparison site; the one permitted comparison emits a log) | It rides the same message as a gating value; a later reader "completing" the comparison silently reinstates exact-version equality and its false refusals | AC 5 |
| Presentation never affects compatibility | Task 6 (digest domain, exclusions named in labelled blocks) | Widening the mod digest to meshes, lights, emitters, or `view_feel` breaks co-op on every cosmetic edit | AC 13, 25 |
| The replicated-slot schema describes declarations both peers are still running | Task 7 (reset on level unload and on a staged commit that changes store declarations) | It is `get_or_insert_with`-cached with no reset today, host and client alike — so a staged reload leaves two peers comparing a fingerprint over declarations neither still has | AC 26, 27 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (world-less frames stay polled) | Later specs key player identity off a connection that must not be re-minted | AC 19, 20 |
| Admission and parity queue independently until their source installs | Task 3 (separate `Option`s, separate early returns, both roles) | Coupling them re-creates the ordering inversion this spec removes | AC 1, 2 |
| A peer refused at admission learns the cause before teardown | Task 3 (deferred disconnect, best-effort) | A future reject path that disconnects inline drops the message entirely | AC 3, 6, 7 |
| No content divergence ever closes a connection | Task 3 (hold at admitted, closing and holding causes separated at the type level) | Any later content check that rejects instead of holding re-creates the disconnect this spec removes, and races an in-flight parity message against a just-installed level | AC 4, 17, 18, 24, 31 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC 29, 30 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `String` on `ModManifestResult` **and `StagedManifest`** | admission variant field | `id: string` | `id: string` |
| mod version | `String` on both, same as above | admission variant field, carried not compared | `version: string` | `version: string` |
| protocol constants | `ProtocolVersion { app_protocol_id: u32, wire_version: u32 }` — `kinematic_static_fingerprint` dropped | admission variant field | n/a | n/a |
| mod compatibility digest | `[u8; 32]`, engine-derived from four `ScriptCtx::data_registry` slices (`entities`, `global_trigger_events`, `global_trigger_pools`, `global_crossings`), re-derived per staged commit | **parity** variant field | n/a (derived) | n/a (derived) |
| level identity (parity) | engine-derived `String` — catalog id, else normalized content-root-relative path | parity variant field | catalog `id` (existing) | same |
| relevel catalog id | `Option<String>` on `NetServer`, installed only for a catalogued level | relevel variant field | catalog `id` (existing) | same |
| level content digest | `[u8; 32]`, widened domain, epoch 2 | parity variant field | n/a | n/a |
| client→server Control envelope | tagged enum in `wire.rs`, mirroring `ClientMessage` | Control, client→server; carries admission + parity | n/a | n/a |
| server→client Control envelope | tagged enum in `wire.rs`, mirroring `ServerMessage` | Control, server→client; carries relevel + divergence | n/a | n/a |
| divergence reason | `DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}` — 2 closing (protocol, mod id), 3 holding (mod digest, level identity, level digest); per-cause payloads pinned in Task 3's table. Carries `Display` + `std::error::Error`; `RejectReason` is deleted | inside the server→client envelope | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed { cause }}` — **stays `Copy`**; the declaration lives beside it, not inside it | not replicated | n/a | n/a |
| `SlotEvent` | `SlotEvent::{Participating { client_id }, Demoted { client_id, cause: HoldingCause }, Closed { client_id, cause: CloseCause }}` — **loses `Copy`** (via `HoldingCause`'s `String`/`[u8; 32]` payload), keeps `Clone` | not replicated | n/a | n/a |
| retained slot declaration | `HashMap<ClientId, ParityDeclaration>` on `NetServer` beside `pending_lifecycle`, cleared on close; `ParityDeclaration { mod_digest: [u8; 32], level_identity: String, level_digest: [u8; 32] }` | not replicated | n/a | n/a |

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
  `context/research/coop-content-compatibility.md`.
- **~~Stable-key sort vs canonical serialization~~ — settled.** Sort by `canonical_name`,
  which is total over the hashed set because descriptors lacking one are excluded from the
  digest entirely and `DataRegistry::upsert_entity_type` guarantees the name unique. Each
  mod-global lane carries its own sort order. The nested question a detail review raised —
  map-valued fields whose iteration order varies per *process* — turned out not to arise:
  no map-valued field is reachable in the domain. The rule survives as a forward-looking
  guard; the determinism hazards that are real here are `f32` bit patterns and the `IrNode`
  walk.
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
- **~~Whether the client should despawn materialized remotes on a mod-digest demotion~~ —
  answered with a default, and the default is the reversible one.** The level-change demotion
  clears the client's world by unloading it; a mod-digest demotion leaves it loaded, so the
  client would keep its `NetworkId→EntityId` map and its remote pawns, which simply stop
  updating. Task 7 despawns mapped remotes and disarms prediction on **both** triggers, so the
  two converge on one client-side state. Recorded rather than closed silently because the
  alternative — accept the frozen tableau — is defensible if the freeze turns out to read
  better than a vanish during a brief hot-reload demotion. Changing it is an edit to one Task 7
  paragraph and AC 28.
- **~~`networking.md` update at promotion~~ — now Task 7 work, not a promotion chore.** The
  fingerprint-binds-the-connection sentence is overturned, the slot lifecycle gains a state and
  a transition, the handshake section describes one app message where there will be three, and
  the crate boundary gains a server→client control message family. Task 7 also writes the
  session-state ledger there, which the roadmap requires and which this spec claimed three
  times without anything delivering it.
