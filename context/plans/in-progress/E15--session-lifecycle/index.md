# Session Lifecycle (E15 Phase 3.75)

## Goal

A client joins a session before it knows which map to load, and the session survives the host
changing levels. Today the app handshake carries the loaded map's fingerprint and neither peer
evaluates it until a level installs, so "connected" and "on the host's map" are one state. A
fingerprint change closes every connection. No message tells a client what to load. The transport
goes unpolled across the unload→install window, so a slow load times peers out regardless.

Split the gate into **admission** (protocol constants and mod identity, no map) and **content
parity** (the level, revalidated at every install). A host level change **demotes** its clients
rather than closing them, names the next map over the wire, and clients follow. The connection
outlives the map it joined on.

## Prerequisites

- **Epic 15 Phase 1** (shipped) — the two-gate handshake, the reliable Control channel, the typed
  reject reason, and the protocol/wire constants this restages.
- **Epic 15 Phase 2** (shipped) — the slot lifecycle, its close events, and the per-slot cleanup a
  demotion reuses.
- **`mod-map-catalog`** (shipped) — the catalog id as the stable logical handle for a map, and
  `LevelSource::Catalog` resolving against the engine-global catalog. The relevel message names a
  catalog id, so the catalog is what makes one string enough.
- **`runtime-level-lifecycle`** (shipped) — the queued load/unload request path a followed relevel
  enqueues into. It scoped "networked or mid-level hot-swap" out; this spec is where that returns.

## Scope

### In scope

- **Two-stage gate.** Admission (protocol constants + mod id) and content parity (mod digest +
  level identity + level digest), as separate control messages evaluated at separate times. Split
  by *mutability*, not by subject: admission carries only values immutable for a connection's
  lifetime, so a mismatch there is terminal. Every content-derived value sits in parity and is
  re-evaluated whenever its source is reinstalled.
- **A slot state between pending and participating.** An admitted slot holds a live connection.
  Both directions are gated: it is sent no entity state, and its inbound traffic is drained and
  discarded, since an undrained reliable channel eventually disconnects the peer it was supposed
  to hold.
- **Participation as a predicate over installed values.** A slot participates exactly while its
  declaration matches the host's installed parity triple. Demotion and promotion are two readings
  of that one comparison. A demotion runs the same per-slot cleanup a close runs today.
- **Mod identity in the manifest** — a stable id that gates admission, and a version that rides
  the wire for display only.
- **Host-replicated tuning.** At the participation transition the host sends the authoritative
  movement descriptor and weapon fire values for that slot's pawn, and the client installs them
  instead of resolving them from its own registry. Hashing is reserved for what cannot be sent
  this way.
- **Two content-derived compatibility digests** — one over the three mod-global registry lanes
  both peers evaluate locally, one over the level's simulated surface — replacing author-declared
  version equality as what gates, only where content cannot be replicated.
- **A relevel message** — server→client, naming the next map's catalog id — and the client-side
  follow that enqueues the load through the shipped request path.
- **Host-side net reset on level unload**, which today early-returns for the host role. Unload
  also clears the host's installed level parity, so a host returning to Frontend demotes its
  clients instead of holding them participating against a world it has torn down.
- **A reserved `CloseCause` variant for a host-initiated leave.** Vocabulary only; nothing emits
  it here.
- **Transport polling across every world-less frame** — keyed on endpoint presence rather than
  boot state — so a load longer than the netcode timeout does not drop every peer, and so a client
  with no level installed can be admitted at all.
- **A typed diagnostic delivered to the client**, distinguishing protocol, mod-identity, and
  content divergence. Protocol and mod-id causes refuse and close; every content cause — mod
  digest, host level absent, level absent, level identity, level digest — is informational and the
  slot holds.
- **Level identity as its own compared value**, and **the fingerprint widened to cover static
  world collision geometry** — closing both of the fail-opens a client can hit.

### Out of scope

- **Player identity, seats, and the roster.** Spec 2 of the band. This spec decides whether a
  *connection* may participate, never who is behind it.
- **Authored join policy.** Spec 3 of the band. Admission is engine mechanism, and the predicate
  that would gate it has nothing to bind against until a roster exists.
- **Reconnect after a close.** A closed connection stays closed. Demotion keeps a connection
  alive; a dropped peer relaunches.
- **Host migration and graceful host-leave.** A later roadmap aspiration. This spec opens the
  session-state ledger but does not serialize it, and reserves the close cause a graceful leave
  will use without implementing the departure.
- **A client asking the host to change level.** Map authority is server-owned. The
  authorized-requester concept arrives with host-as-client packaging (Phase 4).
- **`loadLevel`'s co-op semantics.** The shipped system reaction still loads locally on every
  peer, a connected client included; spec 3 owns the change. Interim behavior is benign under the
  hold rule: a client whose mod loads a different level stops participating, is told why, and
  re-participates at the host's next relevel.
- **Shipping mod content to a client that lacks it.** Networked mod sync stays a non-goal
  (`boot_sequence.md` §8). Matching is in scope; distribution is not, and if it ever lands it
  belongs out-of-band rather than on a reliable game channel. Replicating tuning *values* is a
  different thing and is in scope.
- **Replicating world gravity.** `worldSetGravity` mutates gravity mid-level, so it is not a value
  the host can send once at the participation transition; it needs a continuous replication lane
  this spec does not open. Deferred on mechanism — a reaction-lane hash is still the wrong fix.
- **Tamper resistance.** Mod identity is declared, not proven.
- **Hashing the `.prl` bytes wholesale.** Decided against, not deferred: it turns a cross-platform
  bake difference into a hard connection failure. Widening the fingerprint to the static collision
  it should already have covered is in scope.
- **Frontend/lobby presentation.** A demoted client with no level renders the ordinary world-less
  Frontend state; menus and roster UI are spec 3.

Why each exclusion is an exclusion: `research.md` §Exclusions.

## Direction

**Problem.** The shipped app-gate message packs protocol constants and the loaded map's
fingerprint into one payload, so "connected" and "on the host's map" are the same state and no
state exists for a client that has joined but has not been told what to load.

**Prior commitments.** Preserved: two gates catching different failures at different layers, and
refuse rather than migrate (`context/lib/networking.md`); no entity state reaches a client that
has not passed the gate; author-assigned ids over content-derived ones for *identity*
(`E16--impact-policy-substrate`); the catalog `id` as the stable logical handle
(`mod-map-catalog`); the manifest as the home of mod identity (`M7--mod-script-layer`);
enumerable session state (roadmap Phase 3.75, `context/research/coop-session-lobby.md` §6);
design against host-as-client (roadmap Epic 15); server authority absorbs divergence where a
digest only lets peers refuse each other (`context/research/coop-content-compatibility.md`); the
net crate stays registry-blind. Four divergences, each argued in `research.md` §"Prior
commitments, and four divergences":

1. `context/lib/networking.md`'s "a connection is bound to that fingerprint for its lifetime.
   Installing different static mover content closes it" is overturned — a content change demotes,
   never closes. Rewritten at promotion, ahead of the code.
2. Mod id and version are first-commit-wins across hot reloads, against the atomic-replace
   discipline `mod-map-catalog` sets for manifest lanes. `fonts` is the existing
   non-re-committed lane, so identity joins a minority of one.
3. `context/research/coop-session-lobby.md` §4's declared id-and-version gate is replaced by
   content digests.
4. `context/lib/boot_sequence.md` §8's "networked mod sync" non-goal is read narrowly:
   replicating tuning *values* the host already resolved is server authority, not distribution.

**Alternatives rejected.** The strongest rival is **closing and reconnecting on every level
change**: it preserves the shipped fingerprint binding verbatim and reaches the same end state.
Rejected because no reconnect path exists — the client's handshake flag never resets and the
endpoint is constructed once in `Session::build` — so it is more work than demotion, not less;
and because it turns every level change into a fresh renet_netcode teardown on a direct-connect
transport with no relay, one NAT and handshake failure opportunity per map. Also rejected:
hashing the entity-descriptor closure instead of replicating tuning, a typed wire mirror for the
tuning payload, two specs instead of one, one message evaluated twice, admitting on protocol
alone, gating admission on the mod digest, deferring the mod digest, no new slot state, folding
level identity into the fingerprint, and a content hash over the whole mod. Argued in
`research.md` §"Rejected while drafting" and the sections it links; read it before reopening one.

**Foreclosures.** A single mod id/version pair on the wire forecloses mod stacking without a
protocol change — `M7--mod-script-layer` anticipates mod inheritance, and a stack needs an
ordered set with set-comparison semantics rather than a wider field. The one-way door is the
*requiredness* of mod identity — a protocol bump, a manifest migration for every mod, and a
breaking edit to a published SDK type — accepted, because an optional identity is meaningless for
exactly the mods most likely to drift. Full treatment: `research.md` §"Foreclosures and one-way
doors".

## Decisions

Rivals are listed in Direction and argued in `research.md`.

- **Placement.** The gate stays in `postretro-net`: pure comparison over opaque values, beside the
  two gates already there, unit-testable without a socket. Its compared values are
  engine-supplied and installed after construction, as the fingerprint already is — mod identity
  does not exist when `Session::build` constructs the endpoint, because mod init runs later in
  boot. The level-transition half is engine-side by necessity: it drives the shipped
  `LevelRequest` path and touches the registry, neither of which the net crate may see.
- **Compatibility policy is engine-owned.** Which manifest lanes gate co-op is engine Rust with no
  authoring surface. "Do these two peers compute the same thing" has one correct answer, fixed by
  what the engine's own prediction and hit-declaration code reads. Do not add an authoring
  surface.
- **Two control messages, not one, split by mutability.** Admission carries the two protocol
  constants plus the mod id and version; parity carries the mod compatibility digest, level
  identity, and the level content digest. They become true at different times, and they stop being
  true under different conditions. Admission holds only values that cannot change for a live
  connection; everything content-derived is parity, re-evaluated whenever any of its sources is
  reinstalled.
- **Hash only what cannot be replicated.** A digest is a fallback, not a first instrument:
  replication makes peers *agree*, where a digest only lets them refuse each other. Every value a
  client simulates against that the host can send is sent; the digest covers the remainder, which
  is small.
- **Compatibility is a property of content, not a promise by the author.** What gates is
  content-derived — two digests, one per stage. An author-declared compatibility key moves the
  judgement to a human who gets it wrong silently, and fails as prediction fighting rather than a
  clean refusal. Reasoning: `context/research/coop-content-compatibility.md`.
- **Mod id is declared and gates; mod version is declared and does not.** The id is the namespace
  that makes a catalog id resolvable, so it must match. The version is required in the manifest,
  rides the admission message, and serves display and diagnostics only. Exact-version equality
  would block a friend on the previous build over a lighting tweak, a change no client simulates.
- **Mod id validated at parse: "Must be non-empty ASCII, at most 64 bytes, and use only
  `[A-Za-z0-9_.:-]`."** Reused verbatim from the ammo `type` and weapon `creditSource` ids
  (`crates/postretro/src/scripting/primitives/mod.rs`), so one rule has one wording.
  **Namespacing is conventional, not enforced** — no structural validation, no required
  separator; the docs and example show `postretro.dev` and recommend `yourname.modname`. The
  charset matches the catalog id's *role* as a stable logical handle; it inherits no existing
  catalog rule, because `ModMapEntry.id` has none.
- **Both fields required**, in every shipped manifest — a protocol bump and a manifest migration
  for every mod, accepted for the reason in Foreclosures.
- **First-commit-wins for mod id and version under hot reload; the mod digest re-hashes.**
  Identity is frozen because admission is terminal: a mid-session id change would invalidate
  admission decisions already made, with no state to demote those connections to. The digest is
  the opposite case — a staged reload re-commits the trigger and crossing lanes it reads, and
  parity has a recovery path. A staged commit that moves a hashed lane recomputes the digest and
  reinstalls it; the participation predicate below does the rest, demoting slots that stopped
  matching and promoting those that started matching again.
- **The host replicates the values a client predicts with.** At the participation transition the
  host resolves that slot's pawn class and sends its authoritative `PlayerMovementDescriptor` —
  every tuning struct beneath `movement`, minus render-only `view_feel` — plus the four weapon
  fire fields a client reads through `default_weapon`: `range`, `cooldown_ms`, `fire_mode`,
  `resolution`. The client installs them instead of resolving them from its own registry; Task 7
  redirects the two shipped sites that resolve them locally. `view_feel` stays local, so a
  player's own view-feel settings survive the join.
- **The payload is re-sent whenever it changes.** A staged commit that retunes the host's movement
  or weapon values re-resolves each participating slot's payload and re-sends the ones that moved;
  the client installs and keeps playing. Nothing is hashed, so nothing demotes. The trigger is
  debug-build-only, since the staged manifest path is.
- **The host owns tuning, and that is a behavior semantic.** A modder testing a movement change in
  co-op sees the host's values, not their own. Correct for authoritative co-op — a client
  predicting with its own numbers fights reconciliation instead of diverging cleanly — and
  surprising the first time.
- **`canonical_name` is not hashed.** Shipped code already degrades an unknown entity class rather
  than refusing: `materialize_net_mesh_presentation` logs "leaving remote entity transform-only
  (will not render)" and returns `false`, and `materialize_armed_remote_enemy` documents the same
  rule. Hashing it buys no safety and costs a false refusal on every entity type a mod adds.
- **The mod compatibility digest is three mod-global registry lanes, hashed wholesale.**
  `global_trigger_events`, `global_trigger_pools`, `global_crossings`. No per-field categories, no
  disposition table. Crossings are the load-bearing member: both peers *evaluate* them over the
  same replicated slot values, so the divergence is in a computation each peer runs, not in a
  value the host can send.
- **Replicated descriptor values cross as an opaque payload the net crate carries blindly.** A new
  wire pattern: every opaque value on the wire today is a fixed-size `[u8; 32]`, and this one is
  variable-length and engine-serialized. A typed mirror would make the crate learn the descriptor
  vocabulary and break its registry-blindness. The cost: the crate cannot validate what it
  carries, so a malformed payload is the engine's to detect.
- **State-slot parity is not this digest's job.** `ReplicatedSlotSchema` already hashes the
  replicated slot declarations and both peers already compare it, so a second mechanism over the
  same data is duplication. Task 7 fixes its one real defect: it is process-cached with no reset
  and goes stale across a staged reload.
- **The IR walker is load-bearing, not an over-build.** `CrossingCondition::Ir(IrNode)` sits in
  the digest's domain, so `global_crossings` cannot be covered without walking `IrNode`/`IrValue`.
  The IR is hashed **structurally, never by serializing** — serializing auto-covers new variants
  and destroys the compile error that is the enforcement mechanism.
- **Within every reached type the mod digest is a denylist, not an allowlist.** Bind every field by
  exhaustive destructuring and name any skip. An allowlist ("hash the fields a client simulates
  against") is the mechanism that produced the static-collision fail-open this spec exists to fix:
  a field added later defaults to *unhashed*, and no test catches a field you forgot. The
  guarantee covers fields inside the types the recipe **reaches**, not manifest lanes; a new
  **lane** still escapes it, which is why the uncovered set below is named rather than assumed
  empty.
- **Two lanes remain knowingly uncovered.** `reactions` and `events` are not hashed:
  `SequenceTarget::Entity(EntityId)` is a runtime allocation handle rather than content, and —
  decisively — prediction-relevance is keyed by `PrimitiveDescriptor::primitive`, an **open string
  namespace** for which no compile error is reachable. Both ways around it are mechanisms this
  spec rejects: hashing wholesale is a false refusal, a primitive allowlist is the fail-open.
  Consequence: two mods whose reaction lanes differ can pass both gates and diverge on
  locally-simulated state.
- **Level-local script content is covered by neither digest — a third gap.** All four per-level
  lanes — `reactions`, `crossings`, `trigger_events`, `trigger_pools` — sit outside both domains.
  The mod digest reads `DataRegistry`'s mod-global lanes; the level content digest hashes `.prl`
  geometry and mover data. Script-declared per-level content, populated by `setupLevel()`, sits
  between the two schedules. Named, not closed: closing it means deciding whether the level digest can depend
  on script execution, a boot-ordering question this spec does not open.
- **The level content digest is the existing fingerprint, widened to cover static world
  collision.** Client movement prediction and client-authoritative hit declaration both run
  against the local trimesh `CollisionWorld::populate_from_level` builds, and nothing hashes it
  today — the same silent fail-open as the mover-less case, fixed by the rule that put mover
  collision in the fingerprint: a deterministic prediction input belongs in the parity hash.
  `FINGERPRINT_EPOCH` bumps. Replication was never available here — a level's collision geometry
  is megabytes, and the client loads the map itself anyway.
- **Level identity is the catalog id, falling back to the content-root-relative path** for an
  uncatalogued level. It is opaque to the net crate, and it answers a different question from the
  digest — *which map*, not *is the content the same* — so it stays a field beside the hash rather
  than folded into it. A catalog id is **mod-scoped**, so identity alone cannot discriminate: two
  mods may each declare a map `id: "combat-demo"` over different `.prl` files, and two peers on
  different mods compare identity equal. Admission closes that case, not identity.
- **Parity compares all three values and reports which diverged.** Identity mismatch names both
  maps; level-digest mismatch means same map, different content; mod-digest mismatch means the
  peers' simulated surfaces disagree independently of the map.
- **Participation is a predicate, not a pair of transitions.** A slot participates **if and only
  if** its retained declaration matches the installed parity triple, re-evaluated for every slot
  after every parity source install — the level pair and the mod digest. Demotion and promotion
  are two readings of one comparison. The re-evaluation lives in **one function every install
  setter calls**, so a fourth parity source cannot forget it, and it is verified as a property:
  after any install, for every slot, participating iff matching.
- **The parity declaration's level half is `Option`.** Closes a hole: under the precondition "send
  parity only once both parity `Option`s are present," a client that unloads to Frontend
  mid-session clears its own level parity and then sends nothing — so the host's retained
  declaration still matches, the predicate reads participating, and the host keeps snapshotting a
  world-less client indefinitely. `ParityDeclaration` becomes `{ mod_digest: [u8; 32], level:
  Option<(String, [u8; 32])> }`. The client sends parity as soon as its **mod digest** is present,
  carrying `level: None` when it has no level installed, and re-sends with `None` when it unloads.
  Sending on mod digest alone also tells a client on a diverged mod fork that its mod digest
  diverged while it is still level-less in Frontend, before it spends a load on a map that would
  not have helped.
- **The predicate requires a complete installed triple and a `Some` declared level.** A slot
  participates iff the installed triple is complete, the declared level half is `Some`, and all
  three values match. The host's *installed*-side rule — combine the installed values into a
  comparable triple only when both are present — is untouched: the predicate is a comparison, not
  a type equation.
- **A parity mismatch holds the slot at admitted; it never closes it.** A value belongs in
  admission only if a mismatch on it can never become a match: the protocol constants are compiled
  in, and the mod id is frozen at first commit. Every parity value is *designed* to become true
  later — a level digest at the next install, a mod digest at the next staged commit. Closing on
  one would also race this spec's own criteria: a client's parity message for level A can still be
  in flight when the host installs level B, so a host that closed on mismatch would tear down a
  client it demoted one frame earlier. Every content cause is a *diagnostic* to a still-connected
  client, and the deferred-disconnect mechanism serves admission rejects only.
- **The held slot is bounded by the transport, not by the gate.** A peer that never reaches parity
  holds an `Admitted` slot indefinitely, including one that keeps its keepalive alive but never
  sends a matching parity message, which renet's timeout never reaches. Accepted: the case it
  covers is a peer on the *right* mod whose content diverged, which is recoverable by
  construction. A genuinely wrong mod still closes, on the id, at admission.
- **A relevel names a catalog id, so a host on a raw-path level sends none.** Its clients stay
  admitted until they install a matching level themselves. This keeps the documented loopback dev
  recipe working — the catalog is the only shared namespace in which one string resolves on both
  peers.
- **The slot machine holds state; events derive from transitions.** Two rules, computed from the
  old and new state rather than decided inside each mutating method: **cleanup fires on any exit
  from `Participating`**, whatever the destination, and **the pawn spawn fires on any entry**,
  whatever the origin. A demoted slot's ids are no longer proven, whichever trigger fired, so its
  pawn, replication, ownership, command, state-slot, and combat state clear exactly as a close
  clears them — because the slot left `Participating`, not because a particular method ran. The
  connection survives, not the state.
- **This spec opens the session-state ledger with one entry: the connection.** The roadmap
  requires session-surviving state be enumerated rather than accreted. The enumeration is short
  and defined by subtraction, so the next spec's seat and roster are added to a named list rather
  than discovered.
- **The transport is polled on any frame where a net endpoint exists and no `Running` frame ran
  this frame** — Frontend, Loading, and a resumed splash alike, keyed on endpoint presence rather
  than boot state — for transport advance, keepalive, handshake processing, and Control drain. No snapshot
  apply, no state-crossing detection, no simulation tick; there is no world outside `Running`.
  Frontend coverage is what makes the headline case — a client admitted before any level installs
  — reachable at all.
- **A reject sends its reason before disconnecting.** The slot closes immediately so no further
  traffic is honored; only the socket teardown defers one poll, letting the reliable message
  flush. Without it a player on the wrong mod cannot distinguish a version mismatch from an
  unreachable host.
- **Divergence reasons become a typed enum over two closing causes and five holding ones** —
  protocol and mod id close; mod digest, host level absent, level absent, level identity, and
  level digest hold. One enum, so a single `Display` serves every diagnostic; the closing and
  holding sets are distinguishable at the type level rather than by convention, so a later cause
  cannot land in the wrong lane by omission. `RejectReason` is **deleted**, not wrapped.
- **Both protocol constants bump.** New message vocabulary bumps the app protocol id; the changed
  layout bumps the wire version. Independent bumps, both apply.
- **No endpoint, no gate.** Single-player constructs no endpoint, so no gate runs, no relevel is
  sent, and no path branches on player count. Both digests are computed only where an endpoint
  exists. The level recipe now hashes the world collision buffers, and Phase 4's host-as-client
  packaging installs each level in two worlds on one thread, so an unconditional hash is paid
  twice on a host and for nothing in solo play.
- **`CloseCause` reserves a host-initiated leave.** The roadmap models host migration as a
  handoff, which only works on graceful departure, so the lifecycle needs a leave distinguishable
  from a crash or a timeout. This spec does not implement the departure. It reserves the variant,
  at the one moment the close causes are already being reshaped and both protocol constants are
  already bumping — later the same variant costs another bump.

## Acceptance criteria

Ids are stable. They are never reused and never renumbered; a new criterion takes the next free
number in its area. Cite the id, never the position.

Not cited by any Invariants row, and standing alone: AC-GATE-9, AC-MANIFEST-1,
AC-DIGEST-3, AC-DIGEST-6, AC-DIGEST-12, AC-LEVEL-4, AC-BOOT-4, AC-BOOT-5.

Criteria asserting on a log record use `crates/test-log-capture`, already a dev-dependency of all
three crates this spec touches. Each names the exact level — the harness matches levels exactly,
not as a minimum — and the body substring it pins. That substring becomes contract
(`context/lib/testing_guide.md` §3), so pin the shortest phrase that discriminates and leave the
rest of the line editable.

Criteria marked **review gate** are not runnable: a compile-time guarantee, a two-process recipe,
or an absence over an open set. Each names a runnable substitute covering what can be covered; the
gate itself is checked by reading the diff.

- [ ] **AC-GATE-1** — A client connecting to a host with no level installed is admitted, holds
      the connection open, receives no entity records, and is told `HostLevelAbsent` — the cause
      naming no map in either direction.
- [ ] **AC-GATE-2** — It begins receiving entity records once — and only once — its mod digest
      matches the host's **and** host and client have installed levels whose identity and level
      digest both match.
- [ ] **AC-GATE-3** — A client whose declared mod **id** differs is refused, is told the id
      diverged, and receives no entity records.
- [ ] **AC-GATE-4** — A client whose mod **compatibility digest** differs but whose id matches
      **keeps its connection**, holds at admitted, receives no entity records, and is told the
      mod digest — not the id and not the level — is what diverged. It participates again as
      soon as the two digests agree, whichever peer moved.
- [ ] **AC-GATE-5** — A client whose mod **version** differs but whose id and digest match
      **participates normally**. The difference surfaces as a host-side `Info` record naming both
      versions — pinned substring `mod version differs` — and gates nothing: no reject, no
      demotion, no digest movement.
- [ ] **AC-GATE-6** — A client whose protocol constants diverge is refused with a protocol cause,
      not a mod or content cause.
- [ ] **AC-GATE-7** — A client refused at admission — protocol or mod id — observes the typed
      reason before its connection closes. Task 4's router's `Closing` arm is what delivers it.
      The in-memory relay is the gate. The loopback half extends the bounded-poll harness the
      shipped loopback tests already use (`MAX_POLLS` iterations, no blocking wait, in
      `crates/net/src/transport.rs`), asserting on the settled outcome — a starved socket fails
      loudly rather than hanging.
- [ ] **AC-LEVEL-1** — Two maps that differ only in ways the shipped fingerprint ignored — two
      maps with no movers at all but different brushwork, and two maps whose only difference is
      static world collision geometry — produce different **level content digests** while carrying
      the **same level identity**, and a client on one does not participate on the other. Identity
      must not be what discriminates, or the criterion passes with the epoch untouched.
- [ ] **AC-LEVEL-2** — A catalogued level and the same `.prl` loaded by raw path produce
      **different level identities**, and the mismatch is reported by name rather than as a hash
      diff. Two distinct catalog ids within one mod also produce **different level identities**,
      and a client on one does not participate on the other. Exercised against `level_identity`
      directly — which is why Task 7 extracts it, `install_level_payload` needing a renderer.
- [ ] **AC-DIGEST-1** — A client whose **local movement tuning differs from the host's** —
      including a value nested two levels down (`SpeedParams::run`) and one behind an IR wrapper
      (`DashParams::boost_speed`) — participates and predicts **identically to the host**. It
      simulates against the replicated values, so its local descriptor never reaches prediction.
      The client's own `view_feel` is unchanged by the join.
- [ ] **AC-DIGEST-2** — A client whose local `default_weapon` fire tuning differs from the host's
      **installs the host's four replicated values** — `range`, `cooldown_ms`, `fire_mode`,
      `resolution` — and predicts with them; its local values never reach prediction, and the
      divergence demotes nobody. Host-side hit validation is deliberately not what this criterion
      tests: `AuthorizedShot::range` is already filled from the host's own weapon component
      (`crates/postretro/src/sim/mod.rs`), so a criterion phrased around shot acceptance would pass
      against today's tree unchanged.
- [ ] **AC-DIGEST-3** — Adding an entity type to one peer's mod **demotes nobody** and changes no
      digest. A client that receives a remote entity of an unregistered class stays participating
      and leaves that entity transform-only — the shipped degradation, observed through
      `materialize_net_mesh_presentation`'s existing `Warn` record, whose pinned body substring is
      `not registered; leaving remote entity transform-only`. That is the unregistered-class arm;
      the meshless arm beside it logs at `Debug` and is a different case.
- [ ] **AC-DIGEST-4** — Two mods differing only in a mod-global **crossing** — a threshold, an
      edge, or an IR predicate — produce different mod digests; so do two differing only in a
      mod-global trigger event or trigger pool. Structurally different `IrNode` trees must be
      distinguished by a serializer-free structural walk, and two structurally equal trees must
      hash equal.
- [ ] **AC-DIGEST-5** — Two mods differing only in an `entities` entry — `movement`, `weapon`,
      `default_weapon`, `health`, `behavior`, `canonical_name`, or any presentation field —
      produce the **same** mod digest and interoperate. So do two differing only in a `reactions`
      or `events` entry; those lanes are uncovered by decision, and a test pins that as intended.
- [ ] **AC-DIGEST-6** — Declaring the same entries in a different source order produces the same
      mod digest, independently for each of the three mod-global lanes.
- [ ] **AC-DIGEST-7** — The same content hashes to the same mod digest **across processes and
      builds**, including a crossing carrying non-trivial `f32`/`f64` values and an `Ir`
      expression tree — the two determinism hazards actually reachable in the domain. Pinned by a
      committed digest constant, not a spawned subprocess: the constant was computed in a different
      process from every run that checks it, and it additionally catches a recipe change nobody
      meant to make. Blessed through the same env-var handle as the payload fixture, named in the
      failure message.
- [ ] **AC-DIGEST-8** — **Review gate — a compile-time guarantee, so nothing runs it.** Adding a
      field to any of the six lane descriptor types — `ScopedCrossing`, `CrossingDescriptor`,
      `CrossingCondition`, `TriggerEventDescriptor`, `TriggerPoolDescriptor`, `TriggerPoolArm` —
      without touching the digest recipe **fails to compile**; so does adding an `IrNode` or
      `IrValue` variant. Review confirms the sentinel destructures bind every field by name with no
      rest pattern and match every variant with no wildcard; the compiler enforces it thereafter.
      Runnable substitute: AC-DIGEST-4, which proves the reached fields are hashed rather than
      merely bound.
- [ ] **AC-GATE-8** — A client whose level fails parity **keeps its connection**, receives a
      content diagnostic naming the host's map identity, and re-participates at the host's next
      matching install without reconnecting; a same-identity **level-content-digest** mismatch is
      reported as a content divergence rather than an identity one.
- [ ] **AC-LIFECYCLE-1** — A host installing a different catalog level **keeps** its clients
      connected: each drops to admitted, stops receiving entity records, and its pawn,
      replication, ownership, command, state-slot, and combat state are cleared exactly as a
      disconnect clears them.
- [ ] **AC-LIFECYCLE-2** — The per-slot cleanup runs **exactly once per exit from participating**,
      whatever the destination: once for a demotion, once for a close, once — not twice — for a
      slot demoted and then closed, and **not at all** for an `Admitted → Closed`. Asserted
      against the transition, not against the trigger, so a later exit inherits the rule.
- [ ] **AC-LEVEL-3** — Those clients load the host's new map without being told out of band, then
      re-participate — with their client ids unchanged across the whole transition.
- [ ] **AC-BOOT-1** — A level install that takes longer than the netcode timeout does not drop
      any peer; connections established before the load are still connected after it.
- [ ] **AC-LEVEL-4** — A host on a raw-path level sends no relevel and does not disconnect its
      clients. **Review gate** for the second half: the documented two-process loopback recipe —
      both peers launched with the same map path — still reaches participation. Runnable
      substitute: over the in-memory relay, two peers whose identity was derived from the same
      content-root-relative path reach participating with no relevel sent.
- [ ] **AC-MANIFEST-1** — A manifest missing the mod id or version fails mod init with an
      `InvalidArgument` naming the field and the source path — a returned error, not a log, and
      asserted at all four parse sites. So does an id that is empty, over 64 bytes, non-ASCII, or
      carries a character outside `[A-Za-z0-9_.:-]`. An id with **no namespace separator** — a
      bare `dev` — passes: namespacing is a recommendation. A version passes whatever its shape,
      semver or not, so long as it is non-empty.
- [ ] **AC-MANIFEST-2** — **Debug-build criterion.** A staged hot reload that changes the mod
      **id or version** logs exactly one `Warn` record — pinned substring `mod identity is frozen`
      — and leaves the installed value unchanged, **including in single-player where no endpoint
      exists**; no slot changes state.
- [ ] **AC-DIGEST-9** — **Debug-build criterion.** A staged hot reload that changes a mod-global
      trigger event, trigger pool, or crossing **recomputes and reinstalls** the mod digest;
      participating slots whose declared digest no longer matches drop to admitted with a
      mod-digest diagnostic, none of them is closed, and each demoted slot's pawn, replication,
      ownership, command, state-slot, and combat state are cleared exactly as a level-change
      demotion clears them.
- [ ] **AC-LIFECYCLE-3** — **Debug-build criterion — the recovery direction.** A second staged
      commit that **restores** a previously-matching mod digest — the host reverting its own edit
      — **re-promotes** every held slot whose retained declaration matches again, **with no
      client re-send**: the pawn re-spawns, the tuning payload is re-sent, and snapshots resume.
      The client's declaration never moved; the host's moved twice.
- [ ] **AC-LIFECYCLE-4** — **A property, not a case list.** After any parity install, for every
      slot: the slot is `Participating` **if and only if** the installed triple is complete, the
      declared level half is `Some`, and all three values match. Exercised as a property test over
      generated sequences of installs and declarations across both parity sources, not as an
      enumeration of named transitions.
- [ ] **AC-DIGEST-10** — **Debug-build criterion.** A staged hot reload that changes the host's
      movement or weapon fire tuning **re-sends the payload** to every participating slot, which
      installs it and predicts with the new values. No digest moves and no slot is demoted —
      replication converges where a digest would have refused.
- [ ] **AC-DIGEST-11** — **Debug-build criterion.** A staged hot reload that changes any other
      part of the `entities` lane leaves the mod digest byte-equal, leaves every participating
      slot's `last_sent_tuning` entry unchanged so no payload is sent, and pushes no
      `SlotEvent::Demoted`.
- [ ] **AC-DIGEST-12** — A payload carrying a `TUNING_PAYLOAD_EPOCH` the receiver does not
      recognize leaves prediction inert and demotes nobody, logging an `Error` record naming both
      epochs — pinned substring `tuning payload epoch`. The committed canonical-JSON fixture fails
      when any replicated descriptor field's rendering changes.
- [ ] **AC-BOOT-2** — After a host level change, each of the six level-scoped host tables Task 7
      names — movement owners, slot pawns, replicable set, weapon owners, open shots, command
      queues — holds no entry the unload invalidated, and the replicated-slot schema is rebuilt
      rather than served from the process cache. Enumerated rather than phrased as an absence, so
      the criterion runs.
- [ ] **AC-BOOT-3** — **Debug-build criterion.** A staged reload that adds or removes a store
      namespace rebuilds the replicated-slot schema on **both** host and client rather than
      serving either process cache — so the two peers never compare a schema fingerprint derived
      from declarations neither is running.
- [ ] **AC-LIFECYCLE-5** — A client demoted while participating despawns its mapped remote
      entities and disarms prediction, on both demotion triggers — level change and mod digest —
      rather than retaining a tableau that no longer updates. Both triggers reach
      `NetEndpoint::demote_client_state` with no branch, which is the seam under test. On
      re-promotion it re-arms from the host's next `local_player` baseline. **Review gate** for the
      rest: no promotion message joins the Control variant set, and no client-side latch is added
      for one to clear.
- [ ] **AC-LIFECYCLE-6** — A host that stops holding a level without installing another
      **demotes** every participating slot with the `HostLevelAbsent` cause, which names no map in
      either direction, and closes none. Its clients hold at admitted with no
      level parity installed and no relevel catalog id — the same state a client joining a
      level-less host reaches — and re-participate at the host's next matching install. Exercised
      on **both** paths that reach it through `App::clear_net_level_parity` — which is why Task 7
      extracts it: an unload to Frontend, and a suspend, whose winit callback is unreachable under
      `cargo test`.
- [ ] **AC-LIFECYCLE-7** — A participating client that unloads its level without installing
      another re-declares parity with no level half. The host demotes it with the level-absent
      cause; entity records stop; the connection survives; it re-participates at its next matching
      install.
- [ ] **AC-LEVEL-5** — A redundant relevel naming the level already active or already in flight
      does not restart the load.
- [ ] **AC-LEVEL-6** — A client joining a host that **already has a catalogued level installed**
      receives a relevel for the current map on admission, without waiting for the next
      transition.
- [ ] **AC-LEVEL-7** — A relevel naming a catalog id absent from the client's catalog logs a
      `Warn` record naming the id — pinned substring `relevel names unknown catalog id` — and
      leaves the client admitted rather than closing it.
- [ ] **AC-BOOT-4** — Single-player boot constructs no endpoint and reaches Running unchanged.
- [ ] **AC-BOOT-5** — A host with no mod identity installed — the debug run with no start script —
      logs exactly one `Warn` record at the first connect that arrives, pinned substring `no mod
      identity installed`, and one only however many connects follow. The slot queues at `Pending`
      under the queue-until-installed rule, so without the record the hang is silent.
- [ ] **AC-GATE-9** — A peer built before this change is refused at the transport gate, before any
      app message is decoded. Satisfied by re-staging
      `mover_replay_provenance_wire_version_refuses_previous_peer_on_both_gates` to the new constant
      pair with the previous pair as the refused peer: it hard-asserts both constants, then asserts
      `transport_protocol_id()` differs from the previous composition — gate 1, before any app
      decode — so the previous pair as literals is what "an older peer" means at this gate.
- [ ] **AC-GATE-10** — A slot held at admitted that keeps sending input traffic has its Input
      channel **drained and discarded every poll**: the channel is empty after each poll, and no
      message of any kind — time-sync included — reaches the simulation while the slot is held.
      Asserted over sustained sends across many polls, so received-but-undelivered bytes stay
      bounded rather than accumulating; the `CHANNEL_MEMORY_BYTES` 5 MiB disconnect
      (`crates/net/src/transport.rs`) is the consequence this bounds, not the assertion — reaching
      it in a test would mean pushing 5 MiB through the transport. The client re-converges after
      promotion.

## Tasks

### Task 1: Extract the handshake gate

`crates/net/src/transport.rs` is ~740 non-test lines, and this spec adds a second gate stage plus
a message family to it. Split first, behavior-preserving: move the pure gate surface into a new
`crates/net/src/handshake.rs` beside `slots.rs` — the validation function, the reject reason with
its `Display` and `Error` impls, the protocol-constant accessors (`PROTOCOL_ID`, `WIRE_VERSION`,
`transport_protocol_id`, `protocol_version`), and the malformed/hex helpers. What moves is the
**comparison** surface, not the wire surface: `ProtocolVersion` is a bitcode type and stays in
`wire.rs`, and `transport.rs` contains no wire-serialized type today. `transport.rs` keeps the
renet plumbing: socket, channels, `NetServer`/`NetClient`, the poll loop, and send gating.
`crates/net/src/lib.rs` today holds only `pub mod` declarations; add `pub mod handshake;` beside
`pub mod slots;` so the new module is visible. Re-export **from `transport.rs`** (`pub use
crate::handshake::*;`), not from `lib.rs`: downstream imports are module-qualified — `use
postretro_net::transport::{NetClient, NetServer}` in `crates/postretro/src/netcode/mod.rs`,
`transport::HandshakeOutcome` in `main.rs` — and `lib.rs` carries no re-exports today, so a `pub
use` there mints a new path instead of preserving the existing one. Keeping both — the `pub mod`
in `lib.rs` and the re-export in `transport.rs` — is what makes both paths resolve, and what lets
Task 3 re-export its three types "through `handshake.rs`". Split the existing gate unit tests with
their subject: the pure comparison tests over the validation function and the hex/malformed
helpers move to `handshake.rs`; every test that constructs a `NetServer` or `NetClient` stays in
`transport.rs` — including the relay-pair fixture and the loopback tests Task 3 names.
`mover_replay_provenance_wire_version_refuses_previous_peer_on_both_gates` constructs no
`NetServer`/`NetClient` — it calls `validate_handshake`, `protocol_version`, and
`transport_protocol_id` only — so it moves to `handshake.rs` with the pure comparison tests, not
`transport.rs`; Task 3 re-stages it there. The moved tests reference
`TEST_KINEMATIC_STATIC_FINGERPRINT`, defined today in `transport.rs`'s `mod tests`; duplicate it
into `handshake.rs`'s test module — the two suites are independent after the split. No behavior
change and no acceptance criterion otherwise: it is a move, unchanged in body, with imports
re-pathed. `RejectReason` moves here as part of the behavior-preserving split, and Task 3 deletes
it outright, putting `DivergenceReason` and both impls in `wire.rs`; this placement is
transitional.

### Task 2: Mod identity in the manifest

Add a required stable id and a required version to the mod manifest. Declare both on the
`ModManifest` type at its **registration site** — the `registry.register_type("ModManifest")`
call in `crates/postretro/src/scripting/primitives/manifest.rs::register_sdk_type`, where the doc
strings live. `sdk/types/postretro.d.ts` and the Luau typedef are **generated artifacts** under a
"do not edit by hand" banner, and `committed_sdk_types_match_current_registry` fails CI on a hand
edit; regenerate them with `gen-script-types`, along with the `expected.d.ts` / `expected.d.luau`
fixtures under `crates/postretro/src/scripting/typedef/tests/fixtures/`. **`defineMod` itself is
not generated.** Its declaration and doc comment are hand-written templates —
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` and `sdk_lib.luau` — copied verbatim
by regeneration, and they currently tell authors `config.name` is the only required field. Edit
both, or the shipped SDK keeps documenting `id` and `version` as optional with no CI guard to
catch it. The parity guard in the same file,
`mod_manifest_registered_type_matches_mod_manifest_result`, pins the registered field list to
`ModManifestResult`'s, so it fails the moment the Rust fields land without the registration.
Document what each field is for: peers must declare the **same id** to connect, the **version is
displayed and never compared**, and neither is a security mechanism. Parse both at **four**
sites — the JS and Luau manifest readers in `crates/scripting-core/src/runtime/mod_init_exec.rs`
and their staged-reload counterparts in `crates/scripting-core/src/staged_manifest.rs` — each
following the existing required-`name` shape, so a missing field is an `InvalidArgument` naming
the field and the source path.

**The id rule, in the repo's existing wording.** "Must be non-empty ASCII, at most 64 bytes, and
use only `[A-Za-z0-9_.:-]`." Reuse that sentence verbatim in the registration doc string and in
the hand-written `defineMod` doc comment: it is what the ammo `type` and weapon `creditSource` ids
already say (`crates/postretro/src/scripting/primitives/mod.rs`). Validate at parse — charset
anchored over the whole string, length in bytes not chars, minimum 1 — and compare
case-sensitively at admission. Namespacing is unvalidated: no separator, segment count, or
reserved prefix is checked; say "recommended" in the doc comment, never "must." **The version is
any non-empty string** — no semver parse, no ordering, no range syntax — displayed in logs and
diagnostics only.

Carry both on `ModManifestResult` beside `name` **and on `StagedManifest`**
(`crates/scripting-core/src/staged_manifest/transfer.rs`) — a fifth edit site, and the one the
staged path hands to the main thread. Without it the main-thread commit cannot see the staged
values and the first-wins warning has nothing to compare.

**Own the first-wins comparison and the warning here, not on the endpoint side.** Add a private
committed-identity cell on `ScriptRuntime`, exposed as `pub fn committed_mod_identity(&self) ->
Option<(&str, &str)>` (`crates/scripting-core/src/runtime/core.rs`). Seed it at two sites, not
one. **Seed** inside `ScriptRuntime::run_mod_init`, immediately after the `ModManifestResult` is
stored and before any drain: boot mod init never reaches `commit_staged_manifest_result`, and
`drain_manifest_registrations` plus `run_deferred_mod_init`
(`crates/postretro/src/startup/splash_lifecycle.rs`) `mem::take` the manifest's lanes, so a seed
placed only in the committed-staged path would read an emptied manifest on boot. For a
no-start-script boot, the first committed staged reload seeds it instead. **Compare and warn**
only in `ScriptRuntime::commit_staged_manifest_result`'s committed path: both operands are in
scope there, the staged values ride the built-manifest status and the installed values live on
the same runtime, and a later divergent commit warns and leaves the cell alone. The warning's body
contains `[Scripting] mod identity is frozen`, which AC-MANIFEST-2 pins. The decisive
reason it cannot live on the endpoint side: **AC-MANIFEST-2 must hold in single-player, where no
endpoint exists** — an endpoint-side owner could not satisfy it at all. This applies to
**identity only** — the mod compatibility digest Task 7 installs is re-hashed on every staged
commit. Comment both rules at the commit site together: identity is frozen because admission has
no recovery path, the digest is refreshed because parity does. The freeze diverges
from the atomic-replace discipline most manifest lanes follow — note at the same site that `fonts`
is already non-re-committed, so mod
identity joins an existing minority of one rather than becoming a unique exception. Update all
three committed `defineMod` call sites under `content/` to declare both fields:
`content/dev/start-script.ts`, `content/dev/scripts/frontend-level-select-fixture.ts`, and
`content/dev/scripts/frontend-level-select-fixture.luau`. The `.ts` fixture is a `tsc --noEmit`
review gate — `content/dev/scripts/tsconfig.json`'s `"include": ["./**/*.ts"]` covers
`content/dev/scripts/frontend-level-select-fixture.ts`, so a missing required field there fails
that gate. It does **not** cover `content/dev/start-script.ts`, one directory up: a missing or
malformed field there is caught only at runtime, by the new parse validation, never by CI.

The Script syntax examples block matches that manifest's shape: `maps` imports
`mapCatalog` from `content/dev/scripts/frontend-menu`, because the example mirrors the *runtime*
manifest, which imports its catalog; the two fixtures inline `defineMapCatalog([...])` instead.

### Task 3: Two-stage gate and the slot machine

Replace the single app-gate message with two. **Admission** carries the two protocol constants,
the mod id, and the mod version; **content parity** carries the mod compatibility digest and an
optional level half, `level: Option<(String, [u8; 32])>`, absent while the client has no level
installed. Both compare opaque values the crate never interprets.

**Control needs a tagged envelope first.** `NetServer::process_control_messages` decodes Control
as a bare `let received: ProtocolVersion = wire::decode(&bytes)`. bitcode is not self-describing,
so a second client→server Control message would decode as the first without erroring. The crate
already solved this on `Channel::Input`, and the comment above `ClientMessage` in `wire.rs`
states the rule: the receiver decodes one enum and matches on the variant rather than guessing an
untagged payload's type. Add the Control equivalents as **appended** variants in the same style,
and replace the untagged decode with a match — `ClientControlMessage` client→server, carrying
admission and parity, and `ServerControlMessage` server→client (the name Task 4 already uses),
carrying the divergence diagnostic and the replicated tuning payload. This task defines both
envelopes, the divergence variant, and the tuning variant; Task 4 appends the relevel variant to
the server→client one. Note that at the definition site, so the two tasks do not both claim it.

**The tuning payload is opaque bytes.** Its variant carries a `Vec<u8>` the crate never decodes,
compares, or validates — a new pattern, since every opaque value on the wire today is a
fixed-size `[u8; 32]`. Comment it at the definition site with the reason: a typed mirror would
make this crate learn the descriptor vocabulary and break its registry-blindness, so the payload
is engine-serialized on both ends and this crate is a courier. Task 6 defines the bytes and
Task 7 sends and installs them; nothing in `crates/net` may grow an opinion about them.

**Module placement.** The messages are bitcode wire types, defined in `wire.rs` beside
`ClientMessage`/`ServerMessage` and *compared* in Task 1's `handshake.rs`; `ProtocolVersion` stays
in `wire.rs` too. `DivergenceReason`, `ClosingCause`, and `HoldingCause` are both wire types —
they ride inside the server→client envelope — and the gate's comparison output, so Task 1's split
rule ("what moves is the comparison surface, not the wire surface") does not decide their module
by itself. All three are defined in `wire.rs` with the other bitcode types and re-exported
through `handshake.rs`: when a type is both, the wire side decides. `ProtocolVersion` **drops
`kinematic_static_fingerprint`**, keeping only `app_protocol_id` and `wire_version`, and becomes
the admission variant's constants payload — the fingerprint moves to the parity message, so
leaving the field in place would put a mutable value back in the admission lane. The admission
message carries `String` fields, so it cannot be `Copy` and its constructor cannot stay a
`const fn` the way `protocol_version` is today.

**Slots must retain what they declared.** `SlotTable` is `HashMap<ClientId, SlotState>` — state
only, no payload — and `process_control_messages` drops `received` after comparing. The shipped
fingerprint setter sidesteps this by closing every client on a changed fingerprint rather than
comparing per-slot. Per-slot re-evaluation needs each slot's last-declared `ParityDeclaration`
retained: it is
the left-hand side of the predicate, and promotion needs it as much as demotion does. **Use a
parallel map** — `HashMap<ClientId, ParityDeclaration>` on `NetServer` beside `pending_lifecycle`,
cleared on close — rather than widening the slot record. `SlotState` therefore **keeps `Copy`**;
it gains a variant and nothing else. The parallel map retains a received value and decides
nothing, so it is not a second waiting mechanism. `ParityDeclaration` is the bitcode payload of the
client→server parity Control variant itself, retained by value per slot — the parity message and
the retained declaration share one type, not two shapes kept in sync by hand.

**Delete the already-accepted skip.** `process_control_messages` does not return early for a
client where `self.slots.is_accepted(client_id)` holds — the check is a `continue` inside the
`while let Some(bytes) = self.server.receive_message(client_id, Channel::Control)` loop, under a
comment reading "A client already accepted may send later control traffic," so each message is
received and discarded per-message. Under this design a `Participating` client re-arms and
re-sends parity on **every** level install, so that `continue` swallows precisely the message the
spec depends on. Drain and evaluate Control for slots in `Pending`, `Admitted`, **and**
`Participating`: an admission message from an already-admitted slot is ignored (admission is
once-only per connection), a parity message is re-evaluated on every arrival. The shipped
function-level early return — `let Some(fingerprint) = self.kinematic_static_fingerprint else
{ return outcomes; }` — is **replaced**, not retained: it exists only to build the expected
`ProtocolVersion` on the line below it, and the fingerprint leaves that type. What the
"extend the shipped early-return" rule preserves is its shape, not its body — admission queues
until the mod identity is installed, parity until the mod digest is, each returning early exactly
as the fingerprint guard does today. Concretely: keep the **function-level** early return — do not
call `receive_message` at all while the required installed value is absent — so a message that
arrives before its installed value exists is neither dropped nor evaluated; renet's reliable
buffer holds it until the install. This function-level early return is what makes the reliable
channel the queue, not a buffer this task adds.

The version field is carried for diagnostics and **must not gate**. The only permitted comparison
emits a host-side `info` log naming both versions when an admitted client's version differs from
the installed one; its body contains `[Net] mod version differs`, which AC-GATE-5 pins. Comment it
at the comparison site so a later reader neither "fixes" the missing gate nor deletes the log.

**The divergence reason is a two-level enum**, not a flat one:
`DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}`, with `Display` on the outer so
one impl serves every diagnostic. A flat seven-variant enum would be distinguishable only by
matching on it, which is the convention this replaces. `ClosingCause` covers protocol and mod id;
`HoldingCause` covers mod digest, host level absent, level absent, level identity, and level
digest. Each `ClosingCause` carries expected and received, with the mod-id cause quoting both
peers' declared versions and the level causes distinguishing an identity mismatch from a
same-identity digest mismatch. Each cause's payload is pinned:

| Cause | Lane | Payload |
|---|---|---|
| `Protocol` | closing | `expected: ProtocolVersion`, `received: ProtocolVersion` (two `u32`s each, post-drop) |
| `ModId` | closing | `expected: String`, `received: String`, plus `expected_version: String`, `received_version: String` for the diagnostic |
| `ModDigest` | holding | `expected: [u8; 32]`, `received: [u8; 32]` |
| `HostLevelAbsent` | holding | none — the host has no level installed, so there is no map to name in either direction |
| `LevelAbsent` | holding | `expected_identity: String` — no `received`: a participating client re-declaring with no level has nothing to compare, only the host's map to name |
| `LevelIdentity` | holding | `expected: String`, `received: String` |
| `LevelDigest` | holding | `identity: String`, `expected: [u8; 32]`, `received: [u8; 32]` — identity is carried so the diagnostic can say "same map, different content" |

**`CloseCause` is a different type and gains a reserved variant.** `crates/net/src/slots.rs`
already defines `CloseCause { Disconnect, Timeout }`, used as `SlotState::Closed { cause:
CloseCause }` and `SlotEvent::Closed { cause: CloseCause }`; `ClosingCause` is a distinct type and
both coexist in the crate. `CloseCause` says how a connection ended; `ClosingCause` says why
admission refused it. `CloseCause` gains a third variant for a host-initiated leave. Nothing
emits it in this spec — it is reserved so a graceful departure stays distinguishable from a
crash, at the one point the close vocabulary is open.

The two are distinct variants rather than one retyped: a client re-declaring with no level names
the host's map it fell out of step with; a host with no level installed has no map to name at all,
in either direction. `HoldingCause` carries exactly one cause, so when two or three parity values
diverge at once — the common case on a mod fork that also changed maps — a fixed precedence
decides which is reported: mod digest first, since it is map-independent, then host level absent,
then level absent, then level identity, then level digest; only the first is reported.
`HostLevelAbsent`'s `Display` says the host has no level and that the slot re-participates at the
host's next install. `LevelAbsent`'s `Display` says there is no level installed and names the
host's map.

`RejectReason` is **deleted**. `HandshakeOutcome::Rejected` carries a `ClosingCause` directly, and
the `Display` and `std::error::Error` impls move to `DivergenceReason` — it is a struct with an
`impl std::error::Error` today, and an outer wrapper would leave two spellings of one idea. Its
`ProtocolVersion`-typed fields are also invalidated by the message split. `HandshakeOutcome`
**loses `Copy`; it already derives `Clone`.** The real work is two reuse-after-move sites in
`process_control_messages` — the malformed-decode and validate-failure branches each move `reason`
into `reject` and then reuse it in the outcome — plus two `Some(*reason)` derefs, in
`loopback_diverged_app_version_is_rejected_with_typed_reason` and
`loopback_mismatched_kinematic_static_content_is_rejected_before_snapshots`. `NetServer::reject`'s
signature is `fn reject(&mut self, client_id: ClientId, _reason: RejectReason)` — the parameter is
unused and underscore-prefixed today, so the current moves are free. It is this task's own
requirement that `reject` enqueue the typed cause on Control that makes the parameter live.

**Slot machine — reshaped, not extended.** Add an `Admitted` **variant to `SlotState`** between
`Pending` and `Accepted`, and rename `Accepted` to `Participating`. `SlotEvent::Accepted` is
renamed to `SlotEvent::Participating` the same way — not a new variant left beside a retained
`Accepted`. `SlotEvent::Accepted` is never emitted today; it is matched-but-ignored in
`netcode/mod.rs`, so `SlotEvent::Participating` begins being emitted for the first time.

The shipped table decides each event **inside the method that mutates**: `on_accept` returns
`None` when the slot is already accepted (once-only per `ClientId`) and `on_close` emits only
from `Accepted`. Both are once-only *edges*, and both are wrong once a slot can leave and
re-enter participation. **Compute the event from the old state and the new state instead.** One
private primitive owns every mutation and every emission:

```rust
fn transition(
    &mut self,
    client_id: ClientId,
    next: SlotState,
    holding: Option<HoldingCause>,
) -> Option<SlotEvent>
```

`holding` is `Some` only for `Participating → Admitted`; every other destination carries its
cause in the state or needs none. Two derivation rules replace every special case:

- **Any exit from `Participating` emits the cleanup event** — `Demoted { client_id, cause }` to
  `Admitted`, `Closed { client_id, cause }` to `Closed`. The two share a trigger by construction,
  so "a demotion clears exactly what a close clears" becomes the shape of the code rather than an
  assertion. `Admitted → Closed` emits nothing because no exit from `Participating` occurred, so a
  slot demoted and then closed runs the cleanup exactly once by derivation rather than by a
  once-only method.
- **Any entry to `Participating` emits `Participating { client_id }`** — first admission and
  re-promotion alike. Task 7 hangs the pawn spawn on it, so a re-promoted slot gets a pawn with no
  "must re-emit" rider anywhere.

Besides the derivation, the guards are the primitive's whole body: `Closed` is **terminal** (any
transition out of `Closed` is refused, returns `None`, first cause wins), a transition to the
state the slot already holds is a **no-op returning `None`**, and an unknown client transitioning
to `Closed` is recorded closed and emits nothing. Every idempotence property the shipped table
guarantees survives, derived from the transition table and unit-tested against it.

**`on_accept` and `on_close` disappear as primitives.** "Accept" no longer names a state, and
`on_close` no longer decides anything the primitive does not. Four thin wrappers over
`transition` replace both — `admit`, `participate`, `demote`, `close` — each naming its
destination. `close(client_id, cause: CloseCause)` keeps the shipped `on_close` signature under
the sibling-matching name; rename its call sites in the same pass. `on_connect` is unchanged: it
seeds `Pending` if absent, never resurrects a `Closed` slot, and emits nothing.

**Keep the deprecated accessor aliases.** `NetServer::accepted_clients` and
`NetServer::is_accepted` stay as `#[deprecated]` aliases over the participating-gated accessors,
so `main.rs`, `netcode/mod.rs`, `harness.rs`, and `trigger_state_channel_harness_test.rs` still
compile at the end of Phase 2. Task 7 deletes both.

**Verdict lanes.** `ServerPoll` today carries `handshakes` and `lifecycle`, and `HandshakeOutcome`
is exactly `Accepted`/`Rejected`. A parity *pass* and a parity *hold* map to neither, so the
vocabulary is fixed here rather than left for Task 7 to invent:

| Verdict | Rides | Shape |
|---|---|---|
| admission pass | `handshakes` | `HandshakeOutcome::Admitted { client_id }` |
| admission reject | `handshakes` | `HandshakeOutcome::Rejected { client_id, cause: ClosingCause }` |
| parity hold | `handshakes` | `HandshakeOutcome::ParityHeld { client_id, cause: HoldingCause }` |
| participation transition | `lifecycle` | `SlotEvent::Participating { client_id }` |
| demotion | `lifecycle` | `SlotEvent::Demoted { client_id, cause: HoldingCause }` |

`SlotEvent` derives `Copy` today in `crates/net/src/slots.rs`; `HoldingCause` carries `String` and
`[u8; 32]` payloads, so `SlotEvent::Demoted { client_id, cause: HoldingCause }` costs `SlotEvent`
its `Copy` derive — it keeps `Clone`. `host_handle_lifecycle`
(`crates/postretro/src/netcode/mod.rs`) takes `lifecycle: &[SlotEvent]` and iterates `for event in
lifecycle` over the slice, matching on `&SlotEvent` and dereferencing `*client_id`, so the
exhaustive match arm there gains the demotion variant. The only `SlotEvent`-by-value uses are the
`pending_lifecycle` moves in `crates/net/src/transport.rs`, the `assert_eq!` comparisons in its
relay tests, `NetServer::close_relay_connection`'s `Option<SlotEvent>` return
(`crates/net/src/transport.rs`), and the `&[SlotEvent::Closed { .. }]` slice literal
`crates/postretro/src/netcode/lifecycle.rs` builds in its tests — none of the four requires
`Copy`; all four survive on `Clone` + `PartialEq`.

`ServerPoll` keeps its two vectors — no third field. The split follows the shipped rule:
`handshakes` carries *gate verdicts about a message just evaluated*, `lifecycle` carries *slot
state transitions the engine must clean up after*. `process_control_messages` returns both.
Task 7 hangs the pawn spawn on `SlotEvent::Participating` and the cleanup on `SlotEvent::Demoted`.
The accept signal deliberately rides `handshakes` and not `lifecycle` today, and
`SlotEvent::Accepted` is matched-but-ignored in `crates/postretro/src/netcode/mod.rs` under a
comment saying the arm is kept exhaustive so a new variant is a compile error — so the new
demotion variant breaks that file by design. `main.rs`'s exhaustive match over `HandshakeOutcome`
breaks the same way, by design, for the same reason. Both breaks are minimal arm edits, not a
file-wide pass: in `crates/postretro/src/main.rs`, `HandshakeOutcome::Admitted` is handled exactly
as today's `Accepted` — it keeps calling `replication.register_client`,
`state_slots.register_client`, and `host_handle_accept_descriptor` — and `ParityHeld` is logged
and ignored; in `host_handle_lifecycle`'s `SlotEvent` match, the new `Demoted` arm is an empty
placeholder carrying a `// Task 7 routes this to the close cleanup` comment.

**Installed values, named.** `NetServer` holds the mod identity behind `set_mod_identity(id:
String, version: String)`, and the parity sources behind two setters — `set_mod_digest(Option<[u8;
32]>)` and `set_level_parity(Option<(String, [u8; 32])>)` — because they install on different
schedules: the digest at mod init and at every staged commit, the level pair at every level
install. Combine the installed mod digest and level pair into a comparable triple only when the
level half is present too; a partial install is not a parity value.

**Parity evaluates once the mod digest is installed, not once the triple completes.** Admission
evaluates once the identity is present. Parity is evaluated on every arrival and every install
once the **mod digest** is installed: the level half of the comparison is skipped while the
installed level pair is absent, and every slot is non-participating in that state. This is what
lets a client on a diverged mod fork learn its mod digest diverged while still level-less, and
what lets a host with no level demote and report rather than queue. Both sides extend the shipped
early-return rather than adding a second waiting mechanism.

**`set_kinematic_static_fingerprint` stays as a deprecated alias.** It stays on both roles,
retaining its **shipped semantics**, as a `#[deprecated]` alias Task 7's mechanical pass deletes.
Precisely: `NetServer`'s returns early when the value is unchanged and closes clients only when a
previous value was already installed, so a *first* install closes nobody; `NetClient`'s instead
self-`disconnect()`s when `handshake_sent`. Neither is a regression at the end of Phase 2 — both
are the shipped behavior. This is the same deprecated-alias branch Sequencing already takes for
`accepted_clients`/`is_accepted`, for the same reason: the workspace must compile at the end of
Phase 2.

**One re-evaluation function, called by every install setter.** Both parity setters end by
calling a single `NetServer` method that walks every slot and enforces the predicate:
**participating iff the retained declaration matches the installed triple.** A slot that matches
and is `Admitted` is **promoted**; a slot that no longer matches and is `Participating` is
**demoted**. Neither direction is a feature of a setter, and putting the comparison in one place
is what stops a fourth parity source from implementing half of it. Inlining it per setter loses
promotion: a slot held by a host staged commit would never re-participate even after the host
reverted the edit, because the client's declaration never moves and it has no reason to re-send.
Route the same call from the two client-facing arrival paths as well — a parity message from an
`Admitted` slot and a re-sent one from a `Participating` slot both end in the same comparison. Pin
its signature: `fn reevaluate_parity(&mut self, only: Option<ClientId>) -> Option<HoldingCause>` —
`None` walks every slot, for an install setter; `Some(client_id)` re-evaluates the one slot whose
message just arrived and returns the cause it derived, so the arrival path obtains the cause
without re-deriving the five-cause precedence table. A slot that stays `Admitted` across an install
— the retained declaration still fails to match, unchanged from before the install — gets no fresh
Control diagnostic: the diagnostic rides alongside the `SlotEvent::Demoted` push, so it fires only
on the transition into holding, never on a re-check that confirms a slot already held stays held.

**The re-evaluation function runs outside a poll; it needs a carrier for what it produces, not a
new one.** Three rules pin it down. First, it pushes `SlotEvent::Demoted { cause }` and
`SlotEvent::Participating` onto the shipped `pending_lifecycle` — both `NetServer::update` and
`poll_handshakes` already drain it every poll. `pending_lifecycle` is documented today as carrying
close transitions created outside a poll; this widens its stated role to every install-driven
transition, not just closes. Second, the outbound divergence diagnostic needs no queue of its own:
the re-evaluation function calls `send_message` on `Channel::Control` directly, and renet buffers it
until flushed — the same mechanism `NetServer::send_snapshot` and `send_input` already rely on,
called by the engine outside a poll and flushed by the trailing `transport.send_packets` at the end
of `NetServer::update`. `NetServer::poll_handshakes` does not call `send_packets`; on the relay path
the buffered message leaves via `packets_to_send` instead. Third,
`HandshakeOutcome::ParityHeld` is produced only inside `process_control_messages`, when a
just-arrived parity message is evaluated; an install is not a message and yields no handshake
outcome. That third rule is what keeps the lane split true.

**Gate inbound traffic, not just sends.** `NetServer` owns the drain, following the crate's
registry-blindness rule — draining by slot state needs no registry, and `NetServer::drain_input`
already ships the closed-slot drain-and-drop this extends. The engine's existing per-client drain
keeps its participating gate unchanged; the two never see the same client. A slot below
`Participating` has its Input channel drained and discarded entirely every poll: input commands,
acks, baseline-refresh requests, hit declarations, and time-sync are all dropped, mirroring the
shipped closed-slot drain-and-drop. Time-sync is not exempted: a promoted client re-converges its
clock from the resumed stream exactly as it does on first join, the same recovery argument this
spec already makes for prediction arming, and nothing latches. Put the processed-message
classification in the drain function as a match, so a future Input-channel message is a compile
error rather than a silent omission.

**The drain is mandatory, not an optimization.** `main.rs` drains input only for accepted clients
today, so a held slot's Input channel is never drained at all, and a reliable channel that
overflows its memory budget — `CHANNEL_MEMORY_BYTES`, 5 MiB, under a comment reading "Reliable
channels disconnect on overflow; unreliable channels drop the oldest" — disconnects the peer,
violating this spec's own "no content divergence ever closes a connection" invariant by a route
nothing else guards. That comment does not distinguish send-side from receive-side enforcement,
and renet 2.0.0 is not vendored in this checkout, so confirm receive-side enforcement before
relying on it as the sole justification — the drain is correct regardless, on unbounded memory
growth.

**The demotion pipeline.** An install setter re-evaluates the predicate; a slot that stops matching
pushes `SlotEvent::Demoted { cause }` onto `pending_lifecycle`, and the same re-evaluation function
enqueues the Control diagnostic; the client, on receiving it, despawns its mapped remotes and
clears prediction and replication state without queuing baseline refreshes; the host drops
whatever stale Input was already in flight for that slot. Task 5 and Task 7 reference this
pipeline.

**Send gating and rejects.** Gate `send_snapshot` and the accepted-client accessor on
participating only. `NetServer` has no server→client Control send path today — `send_snapshot` is
Snapshot and `send_input` is Input — so name the new one `NetServer::send_control`. On an
**admission** reject — protocol or mod id — enqueue the typed reason on Control and close the
slot immediately, but defer the socket disconnect to the next poll via a small pending-disconnect
list on `NetServer`. Delivery is best-effort, not guaranteed: `RELIABLE_RESEND` is 300 ms, so at
a 16 ms frame the reason gets exactly one datagram and a single drop loses it. A **parity**
mismatch is not a reject: enqueue the cause as a diagnostic and leave the slot at `Admitted`,
unclosed — every parity value becomes true again at the next matching install or commit.

**Both poll entry points.** `NetServer::update` and `NetServer::poll_handshakes` each
independently drain `pending_lifecycle`, run the `ServerEvent` loop, and call
`process_control_messages` — the `pending_lifecycle` take moves to **after**
`process_control_messages`, not before, so a promotion or demotion the re-evaluation function
pushes in response to a just-arrived parity message rides the same poll's `ServerPoll.lifecycle`
rather than waiting a poll. Only `update` calls `transport.send_packets`, and `poll_handshakes`
is the in-memory relay path `harness.rs` and `transport.rs`'s relay-pair fixture,
`relay_accepted_pair`, feed — used by
`close_relay_connection_surfaces_close_event_and_refuses_traffic`,
`input_from_a_closed_slot_is_ignored`, and `fingerprint_change_surfaces_exactly_one_close_event` —
the real-UDP loopback tests drive `NetServer::update` through `run_handshake`. "The next poll" must
mean both, or relay-driven tests wedge on an undrained pending-disconnect list.

**Client side — declares, doesn't wait for a complete level.** `NetClient` today has exactly one
installed-value setter, `set_kinematic_static_fingerprint`, which stays as a `#[deprecated]` alias
for the same Phase-2-compile reason the server's keeps one, deleted by Task 7's mechanical pass. It
needs `set_mod_identity(id: String, version: String)` for admission and, for parity,
`set_mod_digest(Option<[u8; 32]>)` plus `set_level_parity(Option<(String, [u8; 32])>)` — the same
three shapes as the server, since `ParityDeclaration` and the installed triple share the
pair-with-a-level-option shape. The client compares nothing; it only declares. Its send
precondition is looser than the host's installed-side rule: **send parity as soon as the mod digest
is present**, carrying `level: None` when no level is installed. Task 7 calls all three setters on
both roles; this task only defines them.

**Client side — flags.** Split `handshake_sent` into an admission flag and a parity flag,
replacing today's self-disconnect. The admission flag is **not** "sent once on connect": mod
identity is not installed until mod init, which runs after `Session::build` constructs the
endpoint, so it is sent once on the first poll at which the transport is connected *and* the
identity is present — the server's queue-until-installed rule, applied on the sending side. The
parity flag re-arms whenever either parity source changes: level install, level **clear**, or
staged mod commit — so a client that unloads to Frontend re-sends parity with `level: None` rather
than falling silent. Both flags are duplicated across `NetClient::update` and
`NetClient::update_connections`, so the split lands twice, and the accessor is the
loop-termination condition in `harness.rs`'s `pump_client_to_server`.

Bump `PROTOCOL_ID` and `WIRE_VERSION`, and re-stage the existing both-gates regression test,
`mover_replay_provenance_wire_version_refuses_previous_peer_on_both_gates`, in `handshake.rs` —
which hard-asserts their exact values with bump-specific failure messages — to the new pair, with
the previous pair as the refused peer. That test satisfies AC-GATE-9.

Unit-test the gate and the slot machine without sockets. `crates/net/Cargo.toml` has no
`[dev-dependencies]` section today; add one with `proptest = { workspace = true }` — the
workspace pin is `proptest = "1"` in the root `Cargo.toml`. The slot machine's central test is a
**property**, not a case list: over generated sequences of installs and declarations across both
parity sources, assert that after every install, every slot is `Participating` if and only if the
installed triple is complete, the declared level half is `Some`, and all three values match. Beside
it, pin the two derivation rules
directly — the cleanup event fires once per exit from `Participating` and never on
`Admitted → Closed`, and the participation event fires on every entry including a re-promotion —
plus the worked case the property generalizes: a mod-digest change demoting a participating slot
with no level involved, and the reverting commit promoting it back with no client message in
between.

### Task 4: Relevel message and client-follow

Give the host a way to name the next map and the client a way to follow it. **Append** a relevel
variant to the server→client Control envelope Task 3 defines — Task 3 reserves the slot and
defines no relevel; this task adds it. It carries one map catalog id, sent to every admitted and
participating slot when the host installs a catalogued level, and on admission for a client
joining a host that already has one, so a late joiner is told the current map without waiting for
the next transition. Sending to admitted slots is also what recovers a slot held on a parity
mismatch: it tells a diverged client which map would let it participate.

**The relevel id is a separate installed value, not the parity triple's identity field.** A host
whose active level has no catalog id sends nothing, but the crate cannot evaluate that condition:
level identity is one opaque string it never interprets, and the path fallback is
indistinguishable from a catalog id inside it. So `NetServer` holds an additional `Option<String>`
relevel catalog id behind `set_relevel_catalog_id(Option<String>)`, beside the message it serves,
installed by the engine only when the active level is catalogued and cleared otherwise. **This
setter is server-only** — `NetClient` has nothing to relevel. Both the send-on-install and
send-on-admission paths read that, not the parity value. Send it from the install setter and from
the admission-pass branch of `process_control_messages`, which both poll entry points call; renet
buffers it as it does the divergence diagnostic.

On the client, surface received relevel messages out of the endpoint poll as a typed value the
engine drains. **Retype the existing `NetClient::drain_control`** from `Vec<Vec<u8>>` to
`Vec<ServerControlMessage>` — it has no callers today, so the change costs nothing — rather than
introducing a `ClientPoll` return type, which would mean changing `NetClient::update`'s signature
(`Result<(), _>` today) *and* `update_connections`'s (`()`), touching every call site to carry a
value only this task consumes. **One router serves all four variants.** The drain returns every
server→client Control variant — relevel, the divergence diagnostic in both its `Closing` and
`Holding` arms, and the tuning payload — not just relevel; two drains over one reliable queue would
steal each other's messages. Name the router `client_drain_control`, a `pub(crate)` function in
`crates/postretro/src/netcode/mod.rs`, called from the Running client arm in
`crates/postretro/src/main.rs` and from the world-less poll entry point Task 5 defines,
`NetEndpoint::poll_world_less`. It dispatches by variant: relevel to
`App::enqueue_level_request`, a `Closing` diagnostic to this task's own arm, a `Holding` diagnostic
to Task 7's client-side demotion, and the tuning payload to Task 7's payload install. This task
defines the drain, its relevel arm, and its `Closing` arm — the `Closing` arm logs the typed cause
and surfaces it to the player-facing path the Open questions section names, a client-side
"incompatible host" message; Task 7 adds the `Holding` and tuning-payload arms to the same router,
not a second drain. The drain must run from the world-less frames Task 5 opens as well
as from Running, since a relevel can arrive while an earlier load is in flight. The engine
enqueues `LevelRequest::Load(LevelSource::Catalog(id))` through the shipped request path, which
already unloads any active level first. Ignore a relevel naming the level already active or
already in flight, so a redundant send does not restart a load. An id absent from the local
catalog warns, naming the id, and leaves the client admitted — the recoverable case, since the mods
matched but the catalogs diverged. AC-LEVEL-7 pins the warning's body substring:
`[Net] relevel names unknown catalog id`.

**Mid-load relevel is settled, not open.** `App::enqueue_level_request` already queues a newer
`Load` over a queued one and applies it when the in-flight load completes, so the v1 rule is the
shipped behavior — with one exception: it early-returns with a warning during the **boot** load.
A relevel arriving there is dropped. Scoped rather than fixed: a boot load only runs for a peer
launched with a map argument, which is the loopback dev recipe (AC-LEVEL-4) and not the join flow
this spec opens — a client joining from Frontend has no boot load to race. The dropped case leaves
that client admitted until the host's next transition, recoverable and rare.

### Task 5: Poll the transport across every world-less frame

`net_poll_and_apply` is reached only from the Running gameplay block, via
`frame_order::run_snapshot_apply_stage`. The hole is bigger than the load window: the redraw arm
**returns early on `BootState::Frontend`** before the snapshot-apply stage, and neither
`run_frontend_ui_logic` nor `render_frontend_frame` touches `session.net_endpoint`. The endpoint
is constructed in `Session::build` at boot, so a peer launched with no map argument sits in
Frontend and never advances renet at all — it cannot complete the transport connect, let alone be
admitted. **The spec's headline case is unreachable unless this task covers Frontend**, not just
Loading. Frontend is also where `finish_level_failure` and `unload_level` land.

So: advance the net endpoint from **every world-less frame** — the loading frame in
`crates/postretro/src/startup/lifecycle.rs` and the Frontend path in
`crates/postretro/src/main.rs`. Pin the entry point each site calls: add
`NetEndpoint::poll_world_less(&mut self, dt: Duration, registry: &mut EntityRegistry) ->
WorldLessPoll` in `crates/postretro/src/netcode/mod.rs`, doing exactly the `NetServer::update` /
`NetClient::update` advance, the `pending_lifecycle` drain, and Task 4's `client_drain_control`
Control drain — nothing else; no snapshot apply, no prediction, no command drain. Task 4 and Task
7 both call it by this name. The boundary is stated as both a permission and a prohibition,
since Task 4 drains relevel from these frames and calls `App::enqueue_level_request`, and Task 7's
client-side demotion despawns remotes from them. Forbidden: snapshot apply, state-crossing
detection, the simulation tick — there is no world outside Running, and the snapshot-apply
ordering contract (apply before state-crossing detection, within the Game-logic stage) has no
meaning there. Permitted: transport advance, handshake processing, keepalive, Task 4's Control drain and its
Task-7 handlers, and host slot-lifecycle handling driven by `pending_lifecycle` (the
`host_handle_lifecycle` call, added by Task 7).

Host slot-lifecycle handling belongs on this list because install-driven demotion (Task 3's
re-evaluation function) pushes onto `pending_lifecycle`, and both poll entry points drain it every
poll regardless of boot state — a poll on a Frontend, Loading, or resumed-splash frame that skipped
it would consume and permanently drop an unload- or suspend-driven demotion. What runs there is
bounded by Task 7's two-cleanups distinction: the **per-slot cleanup** keyed by `ClientId` is
world-independent and reachable on a world-less frame; the **level-scoped host table reset** keyed
by level lifetime is bound to a level, and on the unload path it has already run by the time this
poll executes. `host_handle_lifecycle` takes thirteen parameters — `registry, allocator, replicable, replication,
state_slots, slot_pawns, command_queues, owners, weapon_owners, open_shots,
pending_hit_declarations, weaponless_fire_logged, lifecycle` — all but the first and last being
`NetEndpoint::Host` fields, so the world-less call site must hold a `RefCell` borrow of
`script_ctx.registry` while destructuring `Host`. Clone the `ScriptCtx` handle before the
`session.net_endpoint.as_mut()` match, mirroring `install_level_payload`, then call
`script_ctx.registry.borrow_mut()` inside the `Host` arm; the other eleven arguments come from the
`Host` destructure. On the unload path the
despawns are no-ops by the time the event drains, because the drain happens on a later poll, after
`EntityRegistry::clear_for_level_unload` emptied the registry; the stale-id error is already
swallowed. On the **suspend** path the registry is never cleared, so the despawns do real work
there — which is the case that makes threading the borrow mandatory rather than optional.

**The boundary is a predicate on endpoint presence, not a state enumeration.** Implementation: in
`crates/postretro/src/startup/splash_lifecycle.rs`, after `run_splash_frame_zero` or
`run_splash_frame_one` paints succeeds, if `session.net_endpoint` is `Some`, run the same
world-less poll before returning. Naming both matters: frame 0 can loop for many frames on a
transient surface failure, which is the case this paragraph's argument rests on. This does not
move, delay, or conditionalize the splash: during normal boot the session is absent for all of
frame 0 and most of frame 1, so the poll is a no-op and the pixels-first schedule is unchanged —
the endpoint gets polled from the moment it exists, not from a boot-phase boundary. A state enumeration would be wrong: `App::suspended` resets boot state to `Booting` and
re-drives the splash loop from frame 0, but simply never drops the installed `Session` (and the
endpoint it owns) — its own comment states the rule directly: "The rest of the session survives
suspend" (`crates/postretro/src/main.rs`). `install_pending_session`'s `take_once` guard
(`crates/postretro/src/main.rs`) is a different thing: it stops a *rebuild* on the resumed splash,
not what keeps the session alive. The early splash frames can each loop for multiple frames on
transient surface failures, so the window a live endpoint could sit unpolled during a resumed
splash is unbounded. Splash is not excluded; the predicate covers it. One honest limit remains: a
suspension longer than the netcode timeout drops peers regardless, because no frames run at all
while suspended — the predicate's job is only that no *running* frame leaves a live endpoint
unpolled.

Cover the window with a test that holds a load open past the timeout and asserts the connection
survives. Task 7 owns AC-GATE-1's real regression test — a client admitted while the host sits in
Frontend with no level installed — since admission needs mod identity installed on the endpoint,
which Task 7's `NetEndpoint` dispatchers provide; at the end of Phase 2 the client sits at `Pending`
forever.

### Task 6: The content digests and the replicated tuning payload

Pure functions plus their tests; Task 7 installs and sends them. All three live engine-side, so
the net crate keeps treating every value it carries as opaque.

**Level content digest.** Widen `kinematic_static_fingerprint` (`runtime_movers.rs`) to hash the
level's static world collision alongside the mover data it already covers: the
`LevelWorld::vertices` positions and `LevelWorld::indices` buffer that
`CollisionWorld::populate_from_level` builds the client's trimesh from. Hash positions with
`hash_f32` and indices with the new `hash_u32`, both buffers length-prefixed via `hash_len`, in the
same shape the per-mover vertex/index loop already uses, so nothing *in this field* distinguishes
two zero-triangle levels — level identity carries that. Bump `FINGERPRINT_EPOCH` to 2; it is a
function-local `const` inside the recipe, not a shared constant. **Add** the world parameter, do
not substitute it: the signature becomes
`pub(crate) fn level_content_digest(geometry: &KinematicGeometry, world: &LevelWorld) -> [u8; 32]`.
Replacing `&KinematicGeometry` would silently drop the mover list, per-mover collision, and
waypoints the fingerprint covers today; the caller in `install_level_payload` already holds both.
Rename to `level_content_digest` — it is no longer kinematic-only — and carry the rename through
its in-file test call sites and the computation line in `install_level_payload`. It does not touch
`NetEndpoint::set_kinematic_static_fingerprint` (`crates/postretro/src/netcode/mod.rs`) — it
cannot: the new setter takes the identity/digest pair, and identity is derived by Task 7, not this
recipe. Task 7 replaces that dispatcher. The level recipe is knowingly an allowlist over a
compiler-output struct, not a denylist: `LevelWorld`'s remaining fields are either BSP-derived from
the vertex and index buffers already hashed — `cells`, `cell_portal_refs`, `cell_locator_root`,
`cell_locator_nodes`, `portals`, `has_portals`, `bvh` — or presentation-only — `face_meta`,
`texture_names`, `texture_cache_keys`, `lights`, `light_influences` — and hashing them wholesale is
the `.prl`-bytes-wholesale rival this spec rejects, turning a cross-platform bake difference into a
hard connection failure. Test that two levels differing **only** in static
collision produce different digests, that identical geometry with differing entity placements
produces the same one, and that two mover-less levels with different brushwork no longer collide.

**Mod compatibility digest.** Three mod-global registry lanes, hashed wholesale. Nothing from
the `entities` lane: those values are replicated instead, by the payload below.

*Source.* Read from `ScriptCtx::data_registry` after mod init commits, **not** from
`ModManifestResult` — `ScriptingCore::drain_manifest_registrations` takes the manifest's
registrations out by `std::mem::take`, so the manifest is empty by the time anything could hash
it. Read the **mod-global** lanes specifically — `global_trigger_events`, `global_trigger_pools`,
`global_crossings` — not their per-level counterparts. `DataRegistry` holds both sets separately,
the globals populated from the manifest and the per-level ones from `setupLevel()`; only the
globals are mod content. The per-level ones are a named gap, below.

*Domain: three mod-global registry lanes, no per-field categories.* All three are
prediction-relevant and none is presentation, so each lane is hashed entire — nothing to classify
and no disposition table. Per lane, run the exhaustive-destructure walk below for each entry into
a **fresh hasher**, producing a per-entry `[u8; 32]`; sort those digests bytewise; hash the lane
as a length prefix followed by the sorted digests. The sort key is the entry's full canonical
encoding, so it is total by construction, covers every field automatically, and needs no tiebreak.
The compile-error mechanism is untouched: the sort key *is* the destructuring walk, so a field
that fails to compile in the walk also fails in the key. A named-field sort key was rejected:
`tag` is not unique in the trigger-event lane — fan-out declares several entries per tag by design
— though it is for pools, where `drain_trigger_pools_js`
(`crates/scripting-core/src/data_descriptors/js/manifest.rs`) and its Luau counterpart dedupe on
tag via `seen_tags`; and a `slot`-then-`fire` crossing key omits `ScopedCrossing`'s `levels`.

| Lane | Shape | Notes |
|---|---|---|
| `global_trigger_events` | `TriggerEventDescriptor { tag, event, fire, levels }` | all `String`/`Vec<String>`; already derives `Hash` |
| `global_trigger_pools` | `TriggerPoolDescriptor { tag, arm, levels }`, `TriggerPoolArm::{Count(u32), Percentage(f64)}` | needs a **`hash_f64`** helper — only `hash_f32` exists today |
| `global_crossings` | `ScopedCrossing { crossing, levels }`; `CrossingDescriptor { slot: Option<String>, condition, max: f32, edge: Option<String>, fire: Vec<String> }`; `CrossingCondition::{Below{threshold}, Above{threshold}, Ir(IrNode)}` | the `Ir` arm is why the general walker pays for itself |

Crossings are the load-bearing lane, for the same reason they cannot be replicated. Both peers
run the crossing detector over the same replicated slot values, and `context/lib/networking.md`'s
snapshot-apply ordering has a client evaluate crossings over same-frame local slot writes. Two
mods whose thresholds or predicates differ dispatch **different events off identical state**, and
the host has no value it could send to fix that — the divergence is in the computation, not the
input.

*The IR walker.* `CrossingCondition::Ir(IrNode)` puts `postretro_foundation::ir::IrNode` in the
domain — 15 variants (`Const`, `Input`, `Add`, `Sub`, `Mul`, `Div`, `Clamp`, `Lerp`, `Lt`, `Le`,
`Gt`, `Ge`, `Eq`, `Ne`, `Select`), tree-recursive through `Box<IrNode>`, with
`IrValue::{Bool(bool), Number(f32)}` at the leaves. Write `hash_ir_node` and `hash_ir_value` into
the shared helper module rather than inlining the walk at the one call site, so a second IR-valued
field in a hashed lane reuses it instead of growing a second traversal.

**Hash the IR structurally, not by serializing it.** `IrNode`'s serde representation is pinned and
byte-matched, which makes `serde_json::to_vec` tempting, and it would even auto-cover new
variants. Reject it: serializing produces no compile error, so a new variant silently changes
every digest instead of stopping the build, and the compile error is the mechanism this recipe is
built on. Walk it with an exhaustive `match`, no wildcard arm, a discriminant byte per variant,
`Box` recursion, a length-prefixed `hash_str` for `Input { name }`, and `hash_f32` bit patterns
for `IrValue::Number`. The tuning payload below serializes IR instead: auto-coverage is a defect
for a digest and a feature for a payload.

*Two lanes stay uncovered.* `reactions` and `events` are **not** hashed. Two blockers. First,
`SequenceStep::id` is `SequenceTarget::{Entity(EntityId), Activators, FiredTrigger}` and
`EntityId` is a newtype over `u32` — a runtime allocation handle, so hashing it would bind the
digest to spawn order. Second and decisively, whether a reaction is prediction-relevant is keyed
by `PrimitiveDescriptor::primitive`, an **open string namespace** rather than a struct shape. No
compile error is reachable for "someone added a new prediction-relevant primitive." Both escapes
are mechanisms this spec rejects elsewhere: hashing the lanes wholesale demotes every peer when a
`playSound` or `setEmitterRate` argument changes, the Tier 2 false refusal; hashing an allowlist
of primitives is the mechanism that produced the static-collision fail-open. The
`serde_json::Value` payloads are *not* the obstacle — `preserve_order` is off in this workspace,
so `serde_json::Map` is a `BTreeMap` and iterates key-sorted for free.

*A third gap, covered by neither digest.* `DataRegistry` holds per-level
`reactions`/`crossings`/`trigger_events`/`trigger_pools` from `setupLevel()` separately from the
mod-global lanes above. The mod digest reads the globals; the level content digest hashes `.prl`
geometry and mover data. **Level-local, script-declared crossings and reactions fall between
them.** The crossing and trigger halves have no blocker of their own — only no digest that looks
there, because they install on the level schedule rather than the mod one. Closing it is a
level-digest-schedule question this spec does not open.

*State slots are not in this digest.* `ReplicatedSlotSchema`
(`crates/postretro/src/netcode/state_slots.rs`) already hashes every replicated slot's dotted
name, type, range, and scope under its own `FINGERPRINT_STREAM_VERSION`, and both peers already
compare it. Do not build a second mechanism over the same data. The blocker is duplication plus
the private fields a second recipe would have to reach — `SlotRecord`'s `write_generation` and
`StoreDeclarationSet`'s `BTreeMap`. The `IrNode` that `SlotSchema::accumulate` holds is not a
blocker: this recipe walks that enum anyway. This task's only slot-related work is the cache
defect Task 7 fixes.

*Determinism rules.* Enums are hashed through a `match` with **no wildcard arm**. Struct
destructuring gives no exhaustiveness over enums, so this is a separate rule, and it binds
hardest on `IrNode`'s 15 variants and on `CrossingCondition`. `Option` writes a presence byte,
and every string and sequence is length-prefixed via `hash_str`/`hash_len`, so two distinct
descriptor sets cannot concatenate to the same stream. Floats hash as bit patterns through
`hash_f32` (and a new `hash_f64` for `TriggerPoolArm::Percentage`), never through formatting.

Map-valued fields are hashed in **key-sorted order**, and **no map-valued field is reachable in
today's domain** — the three lanes are strings, sequences, scalars, and two enums. The rule stays
as a forward-looking guard, satisfied in advance by the `BTreeMap`-backed JSON payloads should the
reaction lanes ever come in. What makes today's digest cross-process stable is `f32`/`f64` bit
patterns, the `IrNode` walk, and the per-entry digest sort.

*Enforcement.* Within **every** type the recipe reaches — the six lane descriptor types
(`ScopedCrossing`, `CrossingDescriptor`, `CrossingCondition`, `TriggerEventDescriptor`,
`TriggerPoolDescriptor`, `TriggerPoolArm`) and `IrNode`/`IrValue` — bind every field by exhaustive
destructuring. `let Descriptor { a, b, .. }` is forbidden, no rest pattern; enums match with no
wildcard arm. No field in the reached set is skipped, so a new field fails to compile until
someone hashes it, and that compile error is the mechanism. The no-wildcard-arm half has repo
precedent — `compute_fingerprint` over `SlotType` in
`crates/postretro/src/netcode/state_slots.rs`, and the same pattern in `crates/net/src/wire.rs`
and `crates/postretro/src/netcode/movement_state.rs`. The exhaustive-struct-destructure half is
new: neither `kinematic_static_fingerprint` nor `compute_fingerprint` destructures exhaustively
today, both read fields by plain access, and `context/plans/done/E16--source-id-ledger/index.md`
records the churn cost of the pattern this task adopts. The guarantee covers fields inside the
**reached** types, not manifest lanes; a new *lane* still escapes it, which is why the uncovered
set above is named rather than assumed empty.

**A sentinel module gives the compile-error guarantee an artifact.** Add one in the digest
recipe's tests that redundantly destructures all six lane descriptor types and exhaustively
matches `IrNode`, `IrValue`, `CrossingCondition`, and `TriggerPoolArm`. Pin its shape exactly: bind
every field to `_` by name — `let CrossingDescriptor { slot: _, condition: _, max: _, edge: _,
fire: _ } = value;` — never a rest pattern, and enum arms match exhaustively with no wildcard, each
arm destructuring to `_` with an empty body. Naming each field to `_` keeps exhaustiveness (a new
field is still a missing-field compile error) while producing zero bindings, so there are no
unused-variable warnings and therefore no reason for anyone to reach for the `_`-prefixing or rest
pattern that would erode the guarantee. A tuple-consume adds a second list that drifts;
reconstruction doubles the code and looks like it does something, where a sentinel should be
visibly inert. The comment block above it carries the contract: this breaks when a field or
variant is added; the fix is to hash the new field in the recipe and bind it here, never to widen
either pattern. The recipe's own bindings are consumed by hashing, so this bind-every-field-to-`_`
shape applies to the sentinel only.

*Placement, signature, and shared helpers.* Put the recipe engine-side as a sibling of the level
recipe — `crates/postretro/src/mod_digest.rs` — not in `scripting-core`: the manifest lane stays
unaware of netcode, as the mover recipe keeps its byte layout out of the net crate. It is a pure
function over borrowed slices, so Task 7 owns every registry access:

```rust
pub(crate) fn mod_compatibility_digest(
    trigger_events: &[TriggerEventDescriptor],
    trigger_pools: &[TriggerPoolDescriptor],
    crossings: &[ScopedCrossing],
) -> [u8; 32]
```

`hash_len`, `hash_str`, `hash_vec3`, and `hash_f32` are **private free functions inside
`runtime_movers.rs`** today, so a sibling module cannot call them. Move all four, plus the new
`hash_f64` and the IR walker, into a shared `pub(crate)` module —
`crates/postretro/src/content_hash.rs` — that both recipes import. Add `hash_u32` to the moved
set: the moved helpers have no integer entry, `LevelWorld::indices` and `TriggerPoolArm::Count`
are `u32` (the IR discriminant is a `u8`), and the existing recipe hashes indices inline via
`hasher.update(&index.to_le_bytes())`, so without `hash_u32` the shared module ends up with two
spellings of integer hashing. The move is part of this task, not an incidental refactor: without
it the two recipes either duplicate the helpers or diverge.

*Epoch.* A function-local `const MOD_DIGEST_EPOCH: u32 = 1`, mirroring `FINGERPRINT_EPOCH`'s
shape and bumped whenever the recipe changes.

Test: order-insensitivity across all three inputs; that a crossing threshold, edge, or IR
predicate edit moves the digest, and that a trigger-event or trigger-pool edit does; that two
structurally different `IrNode` trees hash differently and two equal ones hash the same; that no
edit to the `entities` lane moves it — `movement`, `weapon`, `default_weapon`, `health`,
`behavior`, `canonical_name`, presentation, in one table-driven test, pinning that the whole lane
is out of the domain by decision; that two mods differing only in a `reactions` or `events` entry
produce the **same** digest, pinning AC-DIGEST-5's uncovered-lanes claim; and that a fixed lane set
carrying non-trivial `f32`/`f64` values and an `Ir` tree hashes to a **committed digest constant**.
That constant is the cross-process check — it was computed in a different process from every run
that reads it — and it wants the same bless handle as the payload fixture: an env-var-gated mode
that rewrites it from the current recipe, named in the failure message alongside the reminder to
bump `MOD_DIGEST_EPOCH` if the recipe change was intentional. Spawning a subprocess would prove
less and cost a test harness.

**Replicated tuning payload.** The engine-side codec for the values the host sends instead of the
client resolving them. Pure encode and decode plus their tests; Task 7 owns both call sites.

Put the payload type and its codec in `crates/postretro/src/netcode/tuning_payload.rs`, engine
side, where the descriptor vocabulary already lives. Name them: `pub(crate) struct TuningPayload`,
`pub(crate) fn encode_tuning_payload(&TuningPayload) -> Vec<u8>`, and `pub(crate) fn
decode_tuning_payload(&[u8]) -> Result<TuningPayload, TuningPayloadError>`, all in this file. It
carries, for one slot's pawn: the
`PlayerMovementDescriptor` with `view_feel` cleared, and the four weapon fire fields reached
through `default_weapon` — `range`, `cooldown_ms`, `fire_mode`, `resolution`. **Cleared, not
removed.** `Option<ViewFeelParams>` carries no `skip_serializing_if`, so the field renders as JSON
`null` and the fixture pins that. Keep the descriptor embedded rather than mirroring its fields
into a payload-local struct: a hand-mirrored copy would silently fail to replicate the next tuning
field someone adds, which is the divergence class this payload exists to close. What guarantees the
host cannot overwrite a client's view feel is the install seam in Task 7, where the client fills
the field from its own local descriptor — a guarantee that survives any later change to the
encoding. Both halves are
`Option`: a pawn class with no `movement` block, or no resolvable `default_weapon`, sends `None`,
and the client leaves that half of prediction inert exactly as it does today when the local
lookup fails.

**The codec has no ordering opinion.** The payload and the pawn's baseline ride different channels
with no relative ordering, so nothing here may assume one arrives before the other. Task 7 owns
the ordering hazard.

Serialize with `serde_json`. The descriptor types and `IrNode` already derive
`Serialize`/`Deserialize` with a pinned representation, `serde_json` is already a `postretro`
dependency, and the payload crosses once per participation transition rather than per frame, so
compactness buys nothing. Unlike the digest, serializing is the *right* instrument here: a new
`IrNode` variant should replicate without anyone editing this file.

**The payload carries its own version, because the protocol bump does not cover it.** The
layout-change-bumps-the-protocol argument does not hold: the payload is `serde_json` over engine
descriptor types, and the shipped rule bumps `WIRE_VERSION` when a *bitcode* byte layout changes.
Adding a field to `PlayerMovementDescriptor` changes no bitcode type, so nothing forces a bump,
and two builds one field apart exchange payloads that decode cleanly and diverge silently. Two
parts close it:

- A `TUNING_PAYLOAD_EPOCH: u32` const, serialized as the payload's leading field and checked at
  decode. A mismatch is a typed error naming both epochs, logged at `error` with a body containing
  `[Net] tuning payload epoch` (AC-DIGEST-12 pins it), prediction left inert — the same degradation
  the spec already ships, now legible instead of silent. Mirror `FINGERPRINT_EPOCH`'s and
  `MOD_DIGEST_EPOCH`'s shape.
- A **committed fixture** pinning a fully-populated payload's canonical JSON, at
  `crates/postretro/src/netcode/tests/fixtures/tuning_payload.expected.json` — the netcode suite's
  own fixtures directory, not the typedef suite's; a netcode payload fixture misfiled under
  `crates/postretro/src/scripting/typedef/tests/fixtures/` would sit in a directory that belongs
  to the typedef tests. It follows the same *shape* as the `expected.d.ts` / `expected.d.luau`
  fixtures — a regenerable artifact whose `git diff` shows old-versus-new field rendering
  directly, not an inline string literal — but `gen-script-types` knows nothing about it, so it
  needs its own regeneration handle: an env-var-gated bless mode on the test (e.g. `BLESS=1 cargo
  test ...` rewrites the fixture from the current encoding), and the failure message names that
  env var as the way to re-bless. The fixture earns its place over a bare hash: the test exists
  for the **unknowing transitive edit** — someone changing a serde attribute or a nested type in
  `crates/foundation` without knowing the descriptor rides the wire — and a hash tells that author
  only that something moved, where the diff names which field's rendering changed. The failure
  message reads as an instruction to the person who just added the field: semantic payload change,
  bump `TUNING_PAYLOAD_EPOCH` and the wire version, or re-bless if the change is non-semantic.

Reject `#[serde(deny_unknown_fields)]` by name. Authored `movement` blocks never reach serde:
`movement_descriptor_from_js` (`crates/scripting-core/src/data_descriptors/js/movement.rs`) and
`movement_descriptor_from_lua` (`crates/scripting-core/src/data_descriptors/lua/movement.rs`) are
hand-written field-by-field converters that ignore unknown keys structurally, so the attribute
could not touch a modder's authored block. `PlayerMovementDescriptor` shares its sub-structs —
`CapsuleParams`, `GroundParams`, `AirParams`, `FallParams`, `DashParams` — with
`PlayerMovementComponent`, and those derived serde impls exist for the component's round-trip —
the doc comment on `NumberOrIr` (`crates/foundation/src/data_descriptors/types/movement.rs`) says
so directly of `DashParams` — so a netcode-motivated attribute placed where it would actually bite
changes behavior on a path this spec does not own. It also interacts badly with the untagged
`NumberOrIr`/`BoolOrIr` representations.

Decode returns a `Result`; the client logs and leaves prediction inert rather than panicking — the
net crate cannot validate what it forwards, so the engine is the only place a malformed payload can
be caught.

Test: round-trip fidelity across every tuning struct, including a `DashParams` field in both
`Literal` and `Ir` form and a nested `SpeedParams::run`; that the encoded form carries no view-feel
tuning whatever the source descriptor held; that both `Option` halves round-trip as `None`; and
that a truncated buffer decodes to an error rather than a panic.

### Task 7: Engine lifecycle wiring

Wire the engine's halves to the new gate. **Mod identity:** read the committed identity through
`ScriptRuntime::committed_mod_identity()` (`crates/scripting-core/src/runtime/core.rs`), which
returns `Option<(&str, &str)>` — the id and version of the first commit that carried them — after
mod init and after each committed staged poll, and install it on the net endpoint through
`set_mod_identity`, reached through the same `session.net_endpoint` borrow `install_level_payload`
uses — mirroring the fingerprint setter's shape. This is mechanical: idempotent first-wins
installation, no comparison and no warning — Task 2's cell already decided which commit wins, this
task only carries the result to the endpoint. A debug run with no start script commits no manifest,
so no identity ever installs: warn once — and once only, however many connects follow — at the
first connect that arrives with no identity installed, so a client queuing at `Pending` forever
under the queue-until-installed rule is a legible failure rather than a silent hang. The body
contains `[Net] no mod identity installed`, which AC-BOOT-5 pins.

This wiring is what makes admission reachable at all, so this task owns AC-GATE-1's real
regression test — a client admitted while the host sits in Frontend with no level installed. Task
5's world-less poll opens the window; admission still needs mod identity installed on the endpoint,
which this task provides, so the test cannot pass at the end of Phase 2.

Both roles need setters, not just the host: `NetEndpoint` installs identity and both parity
sources on `NetServer` (which compares) and on `NetClient` (which only declares). Task 7 owns the
four `NetEndpoint` dispatchers this wiring needs — `set_mod_identity`, `set_mod_digest`, and
`set_level_parity` land on both role arms, so each of those three setters named in Task 3 and Task
4 lands twice here; `set_relevel_catalog_id` dispatches to the server arm only and is a no-op on
the client arm, since `NetClient` has nothing to relevel. Single-player has no endpoint and skips.

**Mod digest:** compute Task 6's recipe over `ScriptCtx::data_registry` — passing
`global_trigger_events`, `global_trigger_pools`, and `global_crossings` as the three slices its
signature takes — and install it through `set_mod_digest` as the parity lane's first source, at
mod init **and again after every staged commit**, since a staged reload re-commits the trigger and
crossing lanes the recipe reads. Compute it only inside a `session.net_endpoint.is_some()` guard,
at both `run_deferred_mod_init` and `App::poll_staged_manifest_results` — single-player pays no
hash. The staged seam is `App::poll_staged_manifest_results`
(`crates/postretro/src/startup/staged_manifest_lifecycle.rs`). The entire staged-manifest
mechanism is debug-build-only: `ScriptRuntime::poll_staged_manifest_builds` returns an empty `Vec`
in release, and `ScriptRuntime::commit_staged_manifest_result` wraps its body in
`#[cfg(debug_assertions)]` and otherwise returns one of five outcome variants,
`StagedManifestCommitOutcome::ReleaseNoop` (both in `crates/scripting-core/src/runtime/core.rs`).
Four things the seam forces:

- Install only on `StagedManifestCommitOutcome::Committed`.
- Match exhaustively: the type has **five** variants — `Committed`, `DiscardedStale`,
  `FailedBuild`, `Rejected`, `ReleaseNoop` — and `App::poll_staged_manifest_results` reads the
  outcome today only through `matches!(outcome, StagedManifestCommitOutcome::Committed { .. })`,
  so an exhaustive match over all five is new work, not an extension of an existing one.
- Place the install inside a `self.session` borrow scope compatible with the one that function
  already scopes around `commit_staged_manifest_result`, so App methods can re-borrow.
- Take the empty-registry digest, not a skip, for the `Committed` outcome whose `result.status` is
  `StagedManifestBuildStatus::NoStartScript`, which clears lanes — skipping would leave a stale
  value installed.

Reinstalling a digest re-evaluates the participation predicate through Task 3's single
re-evaluation function, which demotes slots that stopped matching **and promotes slots that
started matching again**. There is no separate demotion or promotion trigger to write here: there
is one install, and the predicate follows it.

**Level identity and digest:** derive identity in `install_level_payload` from
`App.active_level_source`, which `retain_active_level_tags_for_install` populates on the line
immediately before the digest is computed. The catalog id when present. The path fallback needs
real work: `resolve_level_source`'s `Path` arm stores `map_path.to_string_lossy()`, the raw CLI
argument exactly as typed, absolute or CWD-relative, so two peers launched from different working
directories would diverge on identity while running the same file. Relativize against
`App.content_root`, emit forward slashes, case-sensitive, no `.`/`..` segments, and state the
behavior for a path outside the content root: use the normalized absolute path, and accept that
it only matches a peer launched identically. This normalization is a pure helper, not inline work
in `install_level_payload`: `fn level_identity(source: &LevelSource, content_root: &Path) ->
String` in `crates/postretro/src/startup/lifecycle.rs`, called from `install_level_payload` —
`install_level_payload` itself needs `self.renderer` and cannot run under `cargo test`, so the
normalization must live where it is testable without one. Install it alongside Task 6's widened
level digest,
computed from the same `world` already in scope, through `set_level_parity(Some((identity,
digest)))` on both roles' `NetEndpoint` arms. Also install the relevel catalog id Task 4 needs
through `set_relevel_catalog_id`, set when the source is `Catalog`, cleared otherwise.

Both installs happen only where an endpoint exists. `install_level_payload` computes the
fingerprint today before it tests for one; move the computation inside that test, since Task 6's
recipe now walks the world collision buffers and Phase 4 installs every level twice on a host.

**Unload clears the level parity.** The same `set_level_parity(None)` call, and
`set_relevel_catalog_id(None)`, made from the unload path beside the host-role reset below.
Frontend-with-clients-connected is a session state this
spec already reaches from the join side — a client admitted before any level installs — and
returning to it from a loaded level must land in the same place. Clearing routes through Task 3's
one re-evaluation function like every other install, so demotion follows from the predicate and no
unload-specific transition is written.

**Suspend bypasses this path.** `App::suspended` tears down level state —
`clear_surface_lifetime_level_state`, then `reset_boot_state_after_suspend` nulling
`active_level_source` — without calling `unload_level`, and keeps the session and its endpoint
alive across the boot-state reset to `Booting`. Left alone, a resumed host would still hold its
slots participating against a world it no longer has. Route the suspend path through the same
`set_level_parity(None)` and `set_relevel_catalog_id(None)` calls the unload path uses, **and**
through the same host-side level-scoped table reset — `reset_level_scoped_client_state`'s host
arm, below — since suspend calls `clear_surface_lifetime_level_state`, not `unload_level`, and that
host arm otherwise never runs on this path. So routed, a suspend leaves the same net-endpoint and
host-table state an unload does. `App::suspended` takes `&ActiveEventLoop`, so its clears cannot
be exercised under `cargo test` directly: extract them into `pub(crate) fn
clear_net_level_parity(&mut self)` on `App`, called from both `unload_level` and `suspended`, so
the clears themselves are testable independent of the winit callback.

**Participation seam:** the `SlotEvent::Participating` handler calls
`replication.register_client(client_id)` and `state_slots.register_client(client_id)` **before**
the pawn spawn, on every entry — first admission and re-promotion alike, so "any exit from
`Participating` removes, any entry registers" is symmetric by derivation exactly as the spawn is.
The pawn spawn currently keyed off the accept outcome in `main.rs` moves to this same handler; a
pawn needs a level, which is what parity now proves. That event fires on **every** entry to
`Participating` under Task 3's derivation rule, so a re-promoted slot is re-registered and spawned
a fresh pawn by the same code path that handles a first-time one. This site needs no re-promotion
branch and must not grow one.

**Replicated tuning — the host half.** At the same participation transition, immediately after
the pawn spawn, resolve that slot's pawn class against the `EntityTypeDescriptor`
`host_handle_accept_descriptor` resolved for that slot's spawn point, build Task 6's
`TuningPayload` via `encode_tuning_payload`, and send it on Control through
`NetServer::send_control`. The host resolves; the client
never does. Re-send on a staged commit that changes either half for a participating slot's pawn:
recompute each participating slot's payload, compare it against the last one sent, and send only
what moved. Retain it in a `last_sent_tuning: HashMap<ClientId, TuningPayload>` field on
`NetEndpoint::Host`, cleared in the `Closed` and `Demoted` cleanup — a re-participating slot is
sent a fresh payload on its next promotion, so the retained copy is a change detector, not a cache
the client depends on.

**Replicated tuning — the client half.** The payload rides Control (reliable-ordered); the pawn's
`FullBaseline` rides Snapshot (unreliable) — there is no cross-channel ordering, so "install the
decoded payload before the pawn's components are built" is not a precondition this task can lean
on. Whichever half arrives second completes the arm. The client decodes the bytes through
`decode_tuning_payload` and stores the resulting `TuningPayload` behind a **generation counter**,
bumped on every install. `materialize_net_local_movement_component`
stays on the every-applied-record path — it resolves `entity_class` → `canonical_name` →
`descriptor.movement` today, and takes the stored `PlayerMovementDescriptor` instead, calling
`PlayerMovementComponent::from_descriptor` on it — and rebuilds only when the stored generation is
newer than the one it last built from, a no-op otherwise.
`ClientWeaponState::from_local_pawn_descriptor` already retries every frame while its state is
absent, so it keeps that shape unchanged: it returns `None` while the payload half is absent and
picks it up on the frame it arrives. Loss is impossible — Control is reliable-ordered, so the
payload always arrives, at worst one resend interval late. The same generation counter serves
AC-DIGEST-10's staged-retune re-send, so there is one mechanism, not two. This hazard is invisible
on the loopback host-as-client path, where both halves are produced in the same process on the
same frame; it bites only a real remote client, which is why it must be specified rather than
discovered.

`view_feel` is the one field the payload always carries empty, so the client fills it from its own
local descriptor for that class if it has one and leaves it absent otherwise.
`ClientWeaponState::from_local_pawn_descriptor` resolves the pawn class, then `default_weapon`,
then copies four fields; it takes those four from the payload instead. Both sites keep their
current degradation: an absent half logs and leaves that prediction inert, as they already do when
a local lookup fails. Neither site may keep a fallback to the local registry for these values — a
fallback would silently restore the divergence replication exists to remove, and only on the peers
whose content differs. Their unit tests move with them, from local descriptor tables to payloads.

**Demotion:** route `SlotEvent::Demoted` into `host_handle_lifecycle` so it runs the same per-slot
cleanup a close runs — the same cleanup whose first statement, `replication.remove_client`,
mirrors the registration the participation seam adds on entry: any exit from `Participating`
removes, any entry registers, symmetric by derivation, not by convention, so a re-promoted slot
always has a `ClientReplicationState` and receives baselines again. Do not duplicate that cleanup:
the two events are the two exits from `Participating`, derived by Task 3 from one edge, so routing
them to one cleanup is what Task 3's derivation already guarantees. `host_handle_lifecycle` needs
a call site outside the Running block
for this: Task 5's world-less poll drains `pending_lifecycle` into `ServerPoll.lifecycle` on every
world-less frame, but `host_handle_lifecycle` today runs only inside the Running gameplay block in
`main.rs` — add the call at Task 5's three world-less sites too: the loading frame in
`crates/postretro/src/startup/lifecycle.rs`, the Frontend path in `crates/postretro/src/main.rs`,
and the post-splash-paint path, so an unload- or suspend-driven demotion on a world-less frame is
not silently dropped.

**Host unload reset:** `reset_level_scoped_client_state` early-returns for the host role today;
give it a host arm that clears the level-scoped host tables (movement owners, slot pawns,
replicable set, weapon owners, open shots, command queues) whose entries the unload has
invalidated. These are **two different cleanups**: the per-slot close cleanup keyed by `ClientId`
that a demotion reuses, and the level-scoped host table reset keyed by level lifetime. Neither
subsumes the other — a mod-digest demotion runs the first with no level change, a level unload
runs the second for tables no live slot owns.

**Client-side demotion — the client half of Task 3's pipeline.** A client that receives a
`Holding` diagnostic while participating must react, and the two triggers differ: a level-change
demotion clears the client's world by unloading it, a mod-digest demotion leaves the world loaded.
The seam is the `NetworkId → EntityId` map and prediction arming. `reset_level_scoped_client_state`
does **not** already do this, despite its name: `ClientReplication::reset_for_level_unload` clears
the map but never despawns from the entity registry — on the shipped path the entities vanish only
because its caller, `App::unload_level`, clears the registry wholesale immediately after. Split the
reset into two named layers instead.

Layer one, new: `pub(crate) fn despawn_all_mapped(&mut self, registry: &mut EntityRegistry)` on
`ClientReplication`, iterating a snapshot of `self.map.keys()` and calling the existing private
`apply_despawn` per key (`crates/postretro/src/netcode/client.rs`), swallowing stale-id errors as
that path does. Layer two: the existing clears — prediction
armed-state and command history, interpolation, replication caches — **minus** the baseline-refresh
queuing: a demoted client has nothing to repair, and the host drops the requests anyway under this
spec's Input-gating rule.

On any holding diagnostic received while participating, both triggers call
`NetEndpoint::demote_client_state(&mut self, registry: &mut EntityRegistry)`, which calls
`despawn_all_mapped` and then the clear-only reset — this is the client-side half of Task 3's
demotion pipeline, and the two triggers converge on this one function with **no branch**.
Despawn-all-mapped is idempotent, so it
is correct whether or not the registry has already been cleared — it is not a no-op on the
level-change trigger. `App::drain_level_requests` (`crates/postretro/src/startup/lifecycle.rs`)
calls `unload_level` when it pops the queued `Load` **and `self.boot_state ==
BootState::Running`** — the guard is conditional in source, not unconditional — from the top of
`App::drive_boot_state_for_redraw` — the frame *after* the Control drain handled the diagnostic —
so despawn-all-mapped runs first, against a populated map, and does real despawns. The guard is
load-bearing for Task 4's world-less drain: a relevel arriving in Frontend pops a `Load` and
unloads nothing. Because it empties the map, the later `ClientReplication::reset_for_level_unload`
iterates zero known ids and queues zero `BaselineRefreshRequest`s — exactly the
no-refresh-queuing, obtained for free on the level-change path. The shipped level-unload caller
keeps its current refresh-queuing variant for the non-demotion path.

**There is no client-side promotion signal, and none is needed.** Prediction arming is *derived*,
not latched. `ClientReplication::maybe_arm_local_pawn` runs on **every** applied record — full
baseline and delta alike — and hands an `armed_local_pawn` baseline to `ClientPrediction::arm`,
documented "arm (or re-arm)" and idempotent for the same pawn. When the host promotes a slot,
re-spawns its pawn, and snapshots resume, the first `local_player: true` record re-arms the
client. The demotion reset above clears `armed` to `None`; the snapshot stream sets it again. Do
not add a promotion message: there is no latch for it to clear.

**Replicated-slot schema cache.** `HostStateReplication::schema` is built lazily through
`get_or_insert_with` and has no reset path, and the client-side `schema`/`net_schema` cache
identically. Give all of them a reset on level unload and on **every** committed staged manifest
result, unconditionally — not gated on whether store declarations moved.
`App::poll_staged_manifest_results` (`crates/postretro/src/startup/staged_manifest_lifecycle.rs`)
holds `result` alongside `outcome` and already destructures `result.status` in the committed branch
to read `manifest.events`, and `StagedManifestBuildStatus::Built` carries `Box<StagedManifest>`
whose `store_declarations: StoreDeclarationSet` derives `PartialEq` — a change-detecting comparison
is reachable, just not chosen. The decision survives on different grounds:
`SlotTable::plan_reconcile` reports only `StoreReconcilePlan::new_declarations` and cannot see a
**removal**, which AC-BOOT-3 requires; and a bespoke set comparison at the App seam is a second
mechanism to keep correct. The rebuild is cheap; a per-lane change detector would be a second
mechanism to keep correct for a cost this spec is not paying elsewhere. Now that state-slot parity is owned
by `ReplicatedSlotSchema` rather than by the mod digest, a staged reload that adds a namespace
otherwise leaves both peers comparing a fingerprint derived from schema neither is still running.

**The durable contract is already written; verify it rather than author it.** Promotion to `ready/`
landed the architectural layer in `context/lib/networking.md` ahead of this code: the two-stage
gate and its mutability rule, the slot lifecycle and the participation predicate, the
replicate-versus-hash rule with its named uncovered set, level identity versus level content
digest, mod identity, and the session-state ledger. `boot_sequence.md` §4 records that Frontend is
not a peerless state. Read those sections as the contract this task implements. The remaining doc
work is confined to what only the finished code can settle: correct anything the implementation
had to do differently, and re-check the manual loopback recipe's checklist, which quotes a log
string from the pre-split vocabulary.

Keep the `main.rs` edit to redirecting the two tuning-consuming call sites, adding the `Holding`
and tuning-payload arms to Task 4's `client_drain_control` router, plus the deferred
`accepted_clients`/`is_accepted` mechanical
rename pass and the `set_kinematic_static_fingerprint` deprecated-alias deletion Sequencing assigns
this task. Splitting that file is out of scope and explicitly deferred by
`runtime-level-lifecycle`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split every net-crate edit lands in.

**Phase 2 (concurrent):** Task 2, Task 3, Task 5, Task 6. Disjoint in their primary files —
scripting-core, the net crate, the world-less frames, the two hash recipes and the payload codec
— with two crossings named rather than assumed away:

- **Task 3 is not net-crate-local.** Renaming `Accepted` to `Participating` escapes the crate:
  `NetServer::accepted_clients` is called from `crates/postretro/src/main.rs` and
  `crates/postretro/src/netcode/mod.rs`; `NetServer::is_accepted` from
  `trigger_state_channel_harness_test.rs` and, in-crate, twice from `crates/net/src/harness.rs`,
  which needs no deprecated alias but is touched by both Task 1's relocation and Task 3's rename.
  The exhaustive `SlotEvent::Accepted` arm in `netcode/mod.rs` breaks on the new demotion variant
  by design. Task 3 therefore **keeps `#[deprecated]` aliases** for
  `accepted_clients`/`is_accepted` and defers the mechanical rename to Task 7, rather than
  carrying a rename pass over `crates/postretro` in Phase 2. Without one of the two the workspace
  does not compile at the end of Phase 2; the alias branch also resolves one of the file
  collisions below. `set_kinematic_static_fingerprint` takes the same branch, on both
  `NetServer`/`NetClient` and the `NetEndpoint` dispatcher: it stays with its shipped per-role
  semantics, stated in Task 3, as a `#[deprecated]` alias deleted by Task 7's mechanical pass
  rather than removed here.
- **Task 5's regression test crosses into Phase 4.** AC-GATE-1's real regression test — admitting
  a client while the host sits in Frontend with no level installed — needs mod identity installed
  on the endpoint, which only Task 7's `NetEndpoint` dispatchers provide. Task 5 opens the
  world-less window; Task 7 owns the test. At the end of Phase 2 the client sits at `Pending`
  forever, so the test cannot be written until Phase 4.
- **Task 6 has one existing caller.** `kinematic_static_fingerprint` is called today from
  `install_level_payload`, and Task 6 both renames it and changes its parameter, so Task 6 updates
  that call site in place. Task 7 later rewrites the surrounding lines — a two-line overlap, not a
  conflict. `kinematic_static_fingerprint` also has seven in-file call sites (one binding, one
  `assert_eq!` operand, five `assert_ne!` operands) inside `runtime_movers.rs`'s own `#[cfg(test)]
  fn kinematic_static_fingerprint_changes_for_prediction_inputs`, all needing the rename and the
  new `world` argument, so the parameter change is not a one-line edit.
- **Task 6's third output lands in an already-touched file.**
  `crates/postretro/src/netcode/tuning_payload.rs` is new, but its `mod` declaration goes in
  `crates/postretro/src/netcode/mod.rs` — which Task 3 already edits, for the `SlotEvent` arm
  gaining the demotion variant. One more line in a file already on the list.
- **Four same-file collisions inside Phase 2.**

  | File | Colliding tasks | Resolution |
  |---|---|---|
  | `crates/postretro/src/startup/lifecycle.rs` | Task 5 (loading frame), Task 6 (`install_level_payload` call site) | Disjoint functions; merge. |
  | `crates/postretro/src/main.rs` | Task 5, Task 3's rename pass (`accepted_clients`/`is_accepted`) | Resolved by taking the deprecated-alias branch: Task 3 keeps `accepted_clients`/`is_accepted` as `#[deprecated]` aliases — unaffected by the `SlotEvent`/`HandshakeOutcome` compile-break edits below — leaving the mechanical rename pass to Task 7 in Phase 4. |
  | `crates/postretro/src/netcode/mod.rs` | Task 3's `SlotEvent` arm edit, Task 6's `tuning_payload` `mod` declaration | Disjoint regions; merge on their own. |
  | `crates/postretro/src/main.rs` | Task 5 (loading-frame edit), Task 6 (`mod_digest.rs`/`content_hash.rs` `mod` declarations) | Two lines, in a different region from Task 5's edit; merge. |

- **Two compile breaks are unavoidable.** Regardless of which branch Task 3 takes: the exhaustive
  `SlotEvent` arm in `netcode/mod.rs`, and the exhaustive `HandshakeOutcome` match in `main.rs` —
  `main.rs` matches `HandshakeOutcome` over `Accepted { client_id }` and `Rejected { client_id,
  reason }`, and Task 3's Verdict-lanes table renames `Accepted` to `Admitted`, adds `ParityHeld`,
  and retypes `Rejected`'s payload from `RejectReason` to `ClosingCause`, so all three break that
  match. Task 3 makes both compile as minimal arm edits — `Admitted` handled as today's
  `Accepted`, `ParityHeld` logged and ignored, and the `SlotEvent` arm gains the demotion variant
  — rather than a file-wide pass; Task 7 moves the pawn spawn off that `main.rs` arm in Phase 4.

**Phase 3 (sequential):** Task 4 — consumes Task 3's slot states and Control envelope.
**Phase 4 (sequential):** Task 7 — consumes the setters from Task 3, the committed identity
from Task 2, both recipes and the payload codec from Task 6, and the client-follow drain from
Task 4. Its two consuming-site rewrites touch real call sites elsewhere:
`ClientWeaponState::from_local_pawn_descriptor` (`crates/postretro/src/weapon/mod.rs`) is called
from `crates/postretro/src/main.rs`; `materialize_net_local_movement_component`
(`crates/postretro/src/scripting/builtins/net_descriptor.rs`) is called from
`crates/postretro/src/netcode/remote_materialize.rs`,
`crates/postretro/src/netcode/predict_reconcile_harness.rs`, and
`crates/postretro/src/netcode/predict_reconcile_harness_test_fixtures.rs`. `main.rs` is already on
Task 5's and Task 6's lists, so Task 7 is a fourth `main.rs` toucher rather than a clean file.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| No entity state reaches a slot below participating | Task 3 (send gating) | Every new send path must gate on participation, not admission or connection | AC-GATE-1, AC-GATE-3, AC-GATE-4, AC-LIFECYCLE-1 |
| **Admission carries only values that cannot change for a live connection** | Task 3 (lane assignment), Task 2 (identity frozen at first commit) | A compared value placed in admission because it is known early becomes an unrecoverable close the moment it can be reinstalled | AC-GATE-4, AC-DIGEST-9, AC-MANIFEST-2 |
| Content parity is proven for the *current* content, not the joining one | Task 3 (predicate re-evaluated on source replacement), Task 7 (per-install level pair, per-commit mod digest) | A demotion that failed to clear state would leave stale ids addressable; a parity value that stopped being reinstalled would gate on history, and one never *cleared* would gate on a world the host has torn down | AC-LIFECYCLE-1, AC-LIFECYCLE-6, AC-LIFECYCLE-7, AC-LEVEL-3, AC-DIGEST-9 |
| **A slot participates if and only if the installed triple is complete, the declared level half is `Some`, and all three values match** | Task 3 (one re-evaluation function every install setter calls), Task 7 (both install sites route through it) | Specified as a transition instead of an invariant, it becomes one-directional. The level path hides that, because the client re-sends at every install; the mod-digest path has no re-send and no recovery. A fourth parity source that installs without re-evaluating reintroduces it | AC-DIGEST-9, AC-LIFECYCLE-3, AC-LIFECYCLE-4 |
| A demotion clears exactly what a close clears, **host side** | Task 3 (both events derived from one edge: any exit from `Participating`), Task 7 (routed into the existing cleanup) | A shared trigger, not a convention two paths must both honor. Both demotion triggers, level and mod digest, run the one host path; `Admitted → Closed` runs it not at all. The client side has no close cleanup to reuse — Task 7 defines client-side demotion as two layers, despawn-all-mapped then a clear-only reset minus baseline-refresh queuing, since the level-unload reset does not itself despawn | AC-LIFECYCLE-1, AC-LIFECYCLE-2, AC-DIGEST-9, AC-LIFECYCLE-5 |
| **The pawn spawn fires on any entry to `Participating`** | Task 3 (event derived from the transition, not from a once-only method), Task 7 (spawn keyed to `SlotEvent::Participating`) | The shipped `on_accept` is once-only per `ClientId`, correct until a slot can re-enter participation. A re-promotion that emitted nothing leaves a participating slot with no pawn and snapshots flowing about entities the client never spawned | AC-LIFECYCLE-3, AC-LIFECYCLE-4 |
| Level identity discriminates two distinct levels within one mod, and distinguishes addressing modes for the same file — not two distinct levels across mods, which a mod-scoped catalog id cannot discriminate; that cross-mod collision is closed by admission | Task 7 (catalog id, normalized path fallback) | The per-level gate is a no-op if two levels within a mod can collide, and the addressing-mode case is the one that collides in practice | AC-LEVEL-2 |
| The level digest discriminates content the identity cannot | Task 6 (static collision folded in, epoch 2) | Two maps sharing an identity but differing in brushwork must not compare equal — the fail-open the spec exists to close, tested only if identity is held constant | AC-LEVEL-1 |
| **A value a client simulates against is replicated where it can be, hashed only where it cannot, and named as uncovered otherwise** | Task 3 (payload on the wire), Task 6 (payload codec, three mod-global lanes, level digest), Task 7 (host send, client install) | The governing rule, and the one a later widening is most likely to get backwards: reaching for a hash produces a false refusal on a value the host could have sent. Hash only what is a *computation* both peers run — crossings — or too large to send — static collision. The uncovered tail is deliberate: `reactions`, `events`, and the level-local script lanes are uncovered, and a total-coverage claim would be false | AC-LEVEL-1, AC-DIGEST-1, AC-DIGEST-2, AC-DIGEST-4, AC-DIGEST-5, AC-DIGEST-8, AC-DIGEST-11 |
| **A client predicts with the host's tuning, never its own** | Task 7 (both consuming sites read the replicated payload) | The two sites that resolve local descriptors today — movement component materialization and client weapon state — must keep no registry fallback for these values. A fallback fires only on the peers whose content differs | AC-DIGEST-1, AC-DIGEST-2, AC-DIGEST-10 |
| The mod digest describes the content the host is running now | Task 7 (re-hash on every staged commit) | Freezing it gates live connections on a value the reload already replaced, silently, in the builds where co-op is developed | AC-DIGEST-9 |
| The mod digest is stable across processes | Task 6 (float bit patterns, structural `IrNode` walk, per-entry digests sorted bytewise per lane) | The reachable hazards are float formatting and IR traversal order, not map iteration — no map-valued field is in the domain | AC-DIGEST-7 |
| Mod version is carried and never compared | Task 2 (SDK docs), Task 3 (commented at the comparison site; the one permitted comparison emits a log) | It rides the same message as a gating value; a later reader "completing" the comparison reinstates exact-version equality and its false refusals | AC-GATE-5 |
| The replicated-slot schema describes declarations both peers are still running | Task 7 (reset on level unload and unconditionally on every `Committed` staged-manifest result) | `get_or_insert_with`-cached with no reset today, host and client alike, so a staged reload leaves two peers comparing a fingerprint over declarations neither still has | AC-BOOT-2, AC-BOOT-3 |
| A connection's id survives a level change | Task 3 (demote, never close), Task 5 (world-less frames stay polled) | Later specs key player identity off a connection that must not be re-minted | AC-LEVEL-3, AC-BOOT-1 |
| Admission and parity queue independently until their source installs | Task 3 (separate `Option`s, separate early returns, both roles) | Coupling them re-creates the ordering inversion this spec removes | AC-GATE-1, AC-GATE-2 |
| A peer refused at admission learns the cause before teardown | Task 3 (deferred disconnect send, best-effort), Task 4 (client-side delivery through the router's `Closing` arm) | A future reject path that disconnects inline drops the message entirely. The guarantee is not real until Task 4 lands: Task 3 only enqueues the reason and defers the teardown, and nothing reads it client-side until the router exists | AC-GATE-3, AC-GATE-6, AC-GATE-7 |
| No content divergence ever closes a connection | Task 3 (hold at admitted, closing and holding causes separated at the type level; inbound drained for held slots) | Any later content check that rejects instead of holding re-creates the disconnect this spec removes, and races an in-flight parity message against a just-installed level. The subtler threat is not a check at all: a held slot whose inbound channel stops being drained overflows its reliable-channel budget and is disconnected by the transport, so the invariant falls to a path that never decided anything | AC-GATE-4, AC-GATE-8, AC-GATE-10, AC-LIFECYCLE-1, AC-DIGEST-9, AC-LEVEL-7 |
| A relevel never restarts the load it names | Task 4 (active/in-flight suppression) | Late-join and transition both send; a third sender must suppress too | AC-LEVEL-5, AC-LEVEL-6 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod id | `String` on `ModManifestResult` **and `StagedManifest`**; validated at parse — non-empty ASCII, at most 64 bytes, `[A-Za-z0-9_.:-]`; namespacing unvalidated | admission variant field | `id: string` | `id: string` |
| mod version | `String` on both, same as above; any non-empty string, never parsed | admission variant field, carried not compared | `version: string` | `version: string` |
| protocol constants | `ProtocolVersion { app_protocol_id: u32, wire_version: u32 }` — `kinematic_static_fingerprint` dropped | admission variant field | n/a | n/a |
| mod compatibility digest | `[u8; 32]`, engine-derived from three `ScriptCtx::data_registry` slices (`global_trigger_events`, `global_trigger_pools`, `global_crossings`), re-derived per staged commit | **parity** variant field | n/a (derived) | n/a (derived) |
| replicated tuning payload | engine-side type in `crates/postretro/src/netcode/tuning_payload.rs`: `PlayerMovementDescriptor` with `view_feel` cleared — rendered as JSON `null`, not omitted, and refilled from the client's own descriptor on install — plus `range`/`cooldown_ms`/`fire_mode`/`resolution`; both halves `Option`; leading `TUNING_PAYLOAD_EPOCH: u32` checked at decode, mismatch a typed error naming both epochs; client stores the decoded value behind a generation counter, bumped per install and shared with the staged-retune re-send (AC-DIGEST-10) | `Vec<u8>` in a server→client Control variant — **opaque**, and the only variable-length opaque value on the wire. `crates/net` does not decode, compare, or validate it, and must not learn to: a typed mirror would break its registry-blindness | n/a (host-resolved) | n/a (host-resolved) |
| level identity (parity) | engine-derived `String` — catalog id, else normalized content-root-relative path | parity variant field, inside the `level: Option<(String, [u8; 32])>` half, absent while the client has no level installed | catalog `id` (existing) | same |
| relevel catalog id | `Option<String>` on `NetServer`, installed only for a catalogued level | relevel variant field | catalog `id` (existing) | same |
| level content digest | `[u8; 32]`, widened domain, epoch 2 | parity variant field, inside the same `level: Option<(String, [u8; 32])>` half | n/a | n/a |
| client→server Control envelope | tagged enum in `wire.rs`, mirroring `ClientMessage` | Control, client→server; carries admission + parity | n/a | n/a |
| server→client Control envelope | tagged enum in `wire.rs`, mirroring `ServerMessage` | Control, server→client; carries relevel + divergence + tuning payload | n/a | n/a |
| divergence reason | `DivergenceReason::{Closing(ClosingCause), Holding(HoldingCause)}` — 2 closing (protocol, mod id), 5 holding (mod digest, host level absent, level absent, level identity, level digest); per-cause payloads pinned in Task 3's table. Carries `Display` + `std::error::Error`; `RejectReason` is deleted | inside the server→client envelope | n/a | n/a |
| slot state | `SlotState::{Pending, Admitted, Participating, Closed { cause }}` — **stays `Copy`**; the declaration lives beside it, not inside it. `SlotTable`'s only mutating primitive that **emits events** is a private `transition(client_id, next, holding)`; `on_connect` is unchanged and emits nothing. `on_accept` and `on_close` are gone, and `admit`/`participate`/`demote`/`close` are thin wrappers over `transition` | not replicated | n/a | n/a |
| `SlotEvent` | `SlotEvent::{Participating { client_id }, Demoted { client_id, cause: HoldingCause }, Closed { client_id, cause: CloseCause }}` — **loses `Copy`** (via `HoldingCause`'s `String`/`[u8; 32]` payload), keeps `Clone`. Every variant is **derived from the (old, new) state pair**, never decided inside a mutating method: entry to `Participating` emits the first, the two exits from `Participating` emit the other two | not replicated | n/a | n/a |
| retained slot declaration | `HashMap<ClientId, ParityDeclaration>` on `NetServer` beside `pending_lifecycle`, cleared on close; `ParityDeclaration { mod_digest: [u8; 32], level: Option<(String, [u8; 32])> }` — `level` is `None` while the client has no level installed or has unloaded one | is the client→server parity Control variant's bitcode payload, retained by value per slot | n/a | n/a |
| last-sent tuning payload | engine-side, per participating slot; a change detector for the staged-commit re-send, cleared on close and on demotion | not replicated | n/a | n/a |

## Script syntax examples

```ts
// Proposed design — two new required fields; everything else is unchanged. The `maps`
// import mirrors the runtime manifest, `content/dev/start-script.ts`.
import { mapCatalog } from "./scripts/frontend-menu";

export default defineMod({
  // Stable machine identity. Must be non-empty ASCII, at most 64 bytes, and use only
  // [A-Za-z0-9_.:-]. Peers must declare the same id, and this is the only *declared* field
  // that can refuse a join. Namespacing is a recommendation, not a rule: nothing validates
  // the structure, but "yourname.modname" keeps ids collision-free.
  id: "postretro.dev",
  // Any non-empty string. Display only: shown in logs and diagnostics, never compared,
  // never parsed as semver, never ordered, and it cannot refuse a connection. Whether two
  // builds can play together is decided by content — the host replicates the tuning a
  // client predicts with, and hashes the little that cannot be replicated.
  version: "0.4.0",
  name: "Postretro Dev Mod",
  // Unchanged, and now load-bearing for co-op: the host names one of these catalog ids
  // when it changes level, and clients resolve it locally.
  maps: mapCatalog,
});
```

## Open questions

- **Reject-reason delivery is the trimmable part.** It covers admission rejects only — every
  content divergence holds the connection open, so its diagnostic rides an ordinary reliable
  message with no deferral — leaving one pending-disconnect list and one poll on the
  protocol/mod-id path. The rest of the spec works without it. If it fights renet's teardown, the
  fallback is a **client-side** message with no cause — "incompatible host" — never a host-side
  log alone. Under player hosting the host is a player, not an operator, and nobody reads its
  log; the person who needs the reason is the one who could not join. With the mod digest in the
  parity lane, the common mismatch among friends on slightly different builds no longer travels
  this path.

Questions closed while drafting are recorded in `research.md` §Questions closed while drafting.
