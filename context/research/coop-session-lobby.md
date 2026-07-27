# Co-op Session and Lobby — Design Intent

> **Read this when:** planning how a player connects to a hosted session — mod matching, server-chosen maps, session/player identity, or the authoring surface for a lobby.
> **Status:** design intent for Epic 15, not shipped behavior. Nothing here exists in code.
> **Related:** [Networking](../lib/networking.md) · [Boot Sequence](../lib/boot_sequence.md) §4 · [Scripting](../lib/scripting.md) §§11–12 · [Roadmap](../plans/roadmap.md) Epic 15

---

## 1. The gap

Today a co-op session has no join flow. Both peers are launched with the same map on the
command line, the client computes the same fingerprint, and the handshake passes. There is
no lobby, no mod check, no server map authority, and no identity for a player beyond the
pawn they happen to be driving.

Four capabilities are wanted, and they are one design because they share one blocker:

1. Connect naming a **mod**; the server accepts only matching mods.
2. The **server** decides which map loads; clients follow.
3. **Session identity** — who is in the session, stable across level transitions.
4. A **scripting API** for the lobby, including author-decided join policy.

---

## 2. The ordering problem

This is the crux, and it inverts a shipped invariant.

Gate 2 of the handshake carries the static-kinematic fingerprint, and the host queues every
handshake until its own level installs one (`networking.md` §Two-gate handshake). Acceptance
therefore *requires a loaded map on both sides*. But "the server chooses the map" means the
client must connect **before** it knows which map to load. A client cannot fingerprint a map
it has not been told about.

So the handshake splits in two, along a seam it does not have today:

| Stage | Proves | When |
|---|---|---|
| **Admission** | wire/app protocol, mod identity | once, at connect — no map involved |
| **Content parity** | static-kinematic fingerprint | at every level install, for the session's life |

Content parity moving from *once per connection* to *once per level* is the change that
makes everything else possible. It also overturns `networking.md` §Two-gate handshake:
"A connection is bound to that fingerprint for its lifetime. Installing different static
mover content closes it." That rule exists because there is no relevel protocol — closing
the connection is the only safe response to content the peer cannot validate. Give the
session a level-transition protocol and the correct response becomes *follow the host*, not
*disconnect*.

**Two live defects live in this same seam.** They are not extra scope; they are the same work.

- **Co-op level transitions do not exist.** The host runs no net reset on unload, no
  server→client relevel message exists (`ServerMessage` carries time-sync and shot verdicts
  only), clients never follow the host, and there is no reconnect path. The transport is not
  even polled across the unload→install window, so a slow load times clients out.
- **The fingerprint fails open.** It hashes the mover list and mover collision geometry.
  A map with no movers hashes identically to every other map with no movers, so the gate
  passes and no cleanup runs — clients stay attached to a host where their pawns no longer
  exist. Silent stale-state corruption, strictly worse than a disconnect.

---

## 3. Three identities, kept separate

Collapsing any two of these is what makes per-player state resist design. They have
different lifetimes and different trust properties.

| Identity | Minted by | Lifetime | Answers |
|---|---|---|---|
| **Session id** | host, once per hosted run | the session | "which session is this?" |
| **Seat** | host, on admission | the session; survives level transitions | "which participant?" |
| **Player id** | client, once per device | across sessions; persisted client-side | "which human?" |

**Seat** is the durable per-player key the engine keys session-scoped state by. The existing
per-player address (`PlayerId::{Local, Remote}` in the trigger system) stays what it is —
a within-level pawn/connection address, rebuilt each level. E17 explicitly said not to invent
a heavyweight identity system, and this does not replace that one; it adds a parallel key
that outlives the level, which is the property nothing has today.

**Player id is client-asserted.** There is no account service and none is planned — this is
built for groups of friends. A client can claim any player id. That is the same trust posture
as client-authoritative hit declaration (`networking.md` §Combat authority) and it should be
stated plainly rather than dressed up: the player id makes *rejoin restores your progression*
work, and it is not an authentication mechanism.

Seat release on disconnect versus holding it for a rejoin window is a real decision with a
gameplay consequence (drop mid-level, lose the session's accumulated per-player state). It
needs the rejoin key above to be settled first.

---

## 4. Mod match is a compatibility check, not a security check

Two mechanisms, two jobs — conflating them is the obvious mistake.

- **Mod identity is declared.** The manifest declares an id and a version; the client sends
  them at admission; the host compares. This catches honest drift (wrong mod, stale version),
  which is the actual failure mode among friends. It does not catch tampering, and should not
  claim to.
- **Map content is hashed.** The fingerprint stays content-derived because prediction
  correctness depends on byte-level parity of mover authoring, not on anyone's honesty.

A content hash over the whole mod was considered and is wrong here: it breaks every dev
iteration loop (hot reload changes the hash mid-session), makes legitimate client-side
differences fatal, and buys a property — tamper detection — that is an explicit non-goal
(`index.md` §4, anti-cheat).

The manifest carries a mod name today and no id or version. Adding them is small; the
consequence is that mod identity becomes a wire-visible contract.

**Why mod match and server map authority are one spec.** The host names the map by **catalog
id**, not by path — `LevelSource::Catalog` already exists and resolves against the
engine-global map catalog, which survives level unload. A catalog id is only resolvable on
the client because the mods match. The mod check is the precondition that makes one string
sufficient to move a session between levels.

---

## 5. Ownership split — engine nouns, authored verbs

The VM drops after load (`scripting.md` §1). There is no live script at runtime. So the
obvious API — `onJoinRequest(player) => allow | deny` — is **structurally impossible**: it is
a retained closure crossing the FFI, which §1 and §11 forbid outright. Any lobby design that
starts there is already lost.

What the author can own is the shape the engine already uses everywhere else.

| Engine owns (nouns) | Author owns (verbs) |
|---|---|
| Roster, seats, session id | The predicate that gates admission |
| The admission decision itself | The UI that displays the lobby |
| Only the phases netcode must distinguish | The session's phase vocabulary, and reactions on lifecycle edges |
| Map authority and the relevel protocol | Which map the session starts on |
| Connection lifecycle and cleanup | What "ready" means |

**Join policy is an IR predicate, not a callback.** A §11 typed command buffer over an
engine-published **session scope** — a fixed, append-only fact table (player count, session
phase, time in level) plus ambient store slots. The tracer runs once at declaration; the
engine evaluates it per join attempt. This is exactly the shape `onStateCrossing`'s predicate
overload already ships, and a join check needs no iteration: one evaluation, pure and total.

The ambient-slot arm is what makes this expressive without a callback. "No joining after the
boss door opens" is a slot the mod writes from a reaction and the predicate reads. Authored
policy of arbitrary complexity, zero live code.

**Lifecycle edges are reaction addresses.** Player joined, player left, phase changed — named
addresses the engine auto-fires, exactly as `"levelLoad"` works today (`scripting.md` §12).
Each publishes a dispatch scope. The mod's *response* to a join lives here; the *decision*
lives in the predicate. Separating them is what keeps the decision synchronous and the
response deferred.

**Session phase is authored, not an engine enum.** The engine distinguishes only what netcode
needs for correctness — whether a level is installed, whether a load is in flight — and
publishes that as a fact. The session's *own* phases are a mod-declared store slot the join
predicate reads. A lobby-then-match shooter, a persistent hub world, and a campaign with no
lobby at all are validly different games; a Rust enum picks a winner among them. The slot
machinery already ships, so this costs nothing.

**The lobby UI is mostly already shipped.** The frontend hub gives a menu tree, a menu camera,
and an optional background level (`boot_sequence.md` §4). A lobby is the Frontend state with a
live endpoint accepting connections — not a new top-level app state.

---

## 6. Constraints a design must not violate

- **Host-as-client is already committed.** The roadmap's shipping host model is a local
  headless server process plus the host's own client over loopback; `--host` / `--connect` is
  an intermediate dev shape. So map authority must be **server-owned with an authorized
  requester**, never "the host's local load broadcasts." Designing against today's
  listen-server shape bakes in an assumption the roadmap has already overturned — cheap to
  honor now, expensive to retrofit.
- **`loadLevel` changes meaning.** It is a shipped system reaction that today always loads
  locally. In a session it becomes a request the server may refuse, and on a non-authoritative
  client it is inert. That is a semantic change to a published primitive
  (`index.md` §2, primitive surface is a contract).
- **The IR cannot iterate**, and slots hold only numbers and booleans. A roster is a list.
  Nothing in the authoring surface can loop over players, so any per-player fan-out goes
  through tag/activators targeting or an engine-owned projection — never an authored loop.
- **Clients do not write the save file**, and client-authored writes to server slots are a
  stated Phase 3.5 non-goal. A guest's progression reaching their own device reverses a
  shipped rule and is deliberate work, not an assumption a lobby spec may smuggle.
- **The store is never cleared on level unload.** Anything keyed by a pawn id resets at every
  level change while global slots survive.
- **Networked mod sync and mid-level mod hot-swap are non-goals** (`boot_sequence.md` §8).
  Matching mods is in scope; shipping them to a client is not.
- **Session state must be enumerable, not scattered.** What survives a level transition should
  be a named set, because a future host migration is that same set plus a live-world layer,
  handed to a different destination. Level unload already clears the world and keeps the store;
  the risk is a transition built as ad-hoc patches to whichever tables happen to break, which
  works and leaves no boundary anyone can later serialize. Name the boundary — building a
  serializer for it is the later spec's job, not this band's.

---

## 7. Spec sequence

Three specs, in dependency order. The first is engine-only; the second unblocks per-player
mod state; the third is the authoring surface.

1. **Session lifecycle.** Split the handshake into admission and content parity; add mod
   id/version to the manifest; demote rather than close on a level change; the relevel
   message and client-follow; host-side net reset on unload; transport polling across the
   load window; the fail-open fix. The largest spec, and the one that fixes both live
   defects. Drafted: `plans/drafts/E15--session-lifecycle/`.
2. **Seat, session identity, and roster.** The durable per-player key, the client-asserted
   player id, and the engine-published roster facts the UI and the predicate read.
3. **Lobby authoring surface.** The session scope and its join predicate, the lifecycle
   reaction addresses, and the reference lobby in the dev mod.

Spec 1 was first scoped as two — admission, then transitions — and merged after direction
review. The split failed on its own evidence: the admission half had to pull the fail-open
fix across the seam because its central invariant failed silently without it, and both its
headline criteria were claims about surviving a window the other half owned. The work
divides by layer (gate, wire, engine lifecycle), not by capability.

`E16--per-player-currency` is parked on spec 2 — its shapes were all attempts to key
per-player state without a durable identity.

---

## 8. Open questions

- **Roster display has no expressible shape.** Slots are scalars and the IR cannot iterate, so
  a variable-length player list cannot be read by any authoring surface that exists. Either
  the engine publishes indexed projections (capped, and ugly) or the UI gains a repeated-row
  construct fed by an engine-owned collection. This is likely a `ui.md` question, not a
  netcode one, and it is unsolved.
- **Rejoin key and seat-hold window.** Whether a dropped player's seat is held, for how long,
  and what key reclaims it. Depends on how much the client-asserted player id is trusted.
- **Where a session starts.** Whether a host boots into a lobby by default, or the lobby is a
  frontend menu the mod opts into. Affects whether session phase is engine-mandatory.
- **Dedicated server with no local player.** A headless server has no seat 0. Whether the
  authorized requester is a role on a seat or a separate server-side concept.

---

## 9. Non-goals

- Matchmaking, discovery, relay, NAT traversal — direct connect only (`index.md` §4).
- Authentication, anti-cheat, tamper-resistant identity.
- Shipping mod content to a client that lacks it.
- Multiple simultaneous sessions in one process.
- PvP, teams, or any session model other than co-op.
