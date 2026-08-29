# Co-op Session and Lobby — Design Intent

> **Read this when:** drafting the **Lobby authoring surface** spec — the session scope and its
> join predicate, session-phase-as-authored-store-slot, lifecycle reaction addresses, or roster
> display. Mod matching, server-chosen maps, and session/player identity have shipped.
> **Status:** design intent for the one still-open piece of Epic 15's session/lobby work. Session
> lifecycle and seat/session-identity/roster are shipped (`done/E15--session-lifecycle`,
> `done/E15--seat-session-identity-roster`); the Lobby authoring surface is not.
> **Related:** [Networking](../lib/networking.md) · [Boot Sequence](../lib/boot_sequence.md) §4 ·
> [Scripting](../lib/scripting.md) §§11–12 · [Roadmap](../plans/roadmap.md) Epic 15 ·
> `context/plans/done/E15--session-lifecycle` · `context/plans/done/E15--seat-session-identity-roster`

---

## 1. Shipped

Three of the four capabilities this note originally scoped have shipped: mod match at connect,
server-chosen maps (the split handshake, the relevel protocol, host-named map authority), and
session identity that survives a level transition (session id, seat, player id) —
`context/plans/done/E15--session-lifecycle`, `context/plans/done/E15--seat-session-identity-roster`.
The durable shape lives in `context/lib/networking.md`: the two-gate handshake and the
admission-vs-content-parity split (§Two-gate handshake, §Admission and content parity), the
four-stage slot lifecycle (§Slot lifecycle), the session-state ledger enumerating
connection/seat/roster (§Session-state ledger), and the mod-identity id-gates/version-never-gates
split (§Mod identity). The rejoin key and seat-hold window question this note originally left open
is answered there too (seat-hold window + reclaim-by-player-id on rejoin).

One trust-posture line is worth keeping intact here, because a shipped spec cites it directly
(`done/E15--seat-session-identity-roster/index.md`):

**Player id is client-asserted.** There is no account service and none is planned — this is
built for groups of friends. A client can claim any player id. That is the same trust posture
as client-authoritative hit declaration (`networking.md` §Combat authority) and it should be
stated plainly rather than dressed up: the player id makes *rejoin restores your progression*
work, and it is not an authentication mechanism.

What remains is the fourth capability: a scripting API for the lobby, including author-decided
join policy — the **Lobby authoring surface** (open, `roadmap.md` ~line 205).

---

## 2. Ownership split — engine nouns, authored verbs

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

## 3. Constraints for the lobby authoring surface

- **The IR cannot iterate**, and slots hold only numbers and booleans. A roster is a list.
  Nothing in the authoring surface can loop over players, so any per-player fan-out goes
  through tag/activators targeting or an engine-owned projection — never an authored loop.
- **Clients do not write the save file**, and client-authored writes to server slots are a
  stated Phase 3.5 non-goal. A guest's progression reaching their own device reverses a
  shipped rule and is deliberate work, not an assumption a lobby spec may smuggle.
- **Networked mod sync and mid-level mod hot-swap are non-goals** (`boot_sequence.md` §8).
  Matching mods is in scope; shipping them to a client is not. Scripts are small enough to
  send (160K against 337M of art in the dev mod) but sending them fixes only the script-side
  third of the breaking surface, inverts boot ordering, and feeds peer-controlled input to a
  C interpreter — reasoned through in
  [Co-op Content Compatibility](./coop-content-compatibility.md) §5.

---

## 4. Remaining spec

**Lobby authoring surface.** The session scope and its join predicate, the lifecycle
reaction addresses, and the reference lobby in the dev mod.

---

## 5. Open questions

- **Roster display has no expressible shape.** Slots are scalars and the IR cannot iterate, so
  a variable-length player list cannot be read by any authoring surface that exists. Either
  the engine publishes indexed projections (capped, and ugly) or the UI gains a repeated-row
  construct fed by an engine-owned collection. This is likely a `ui.md` question, not a
  netcode one, and it is unsolved.
- **Where a session starts.** Whether a host boots into a lobby by default, or the lobby is a
  frontend menu the mod opts into. Affects whether session phase is engine-mandatory.
- **Dedicated server with no local player.** A headless server has no seat 0. Whether the
  authorized requester is a role on a seat or a separate server-side concept.

---

## 6. Non-goals

- Matchmaking, discovery, relay, NAT traversal — direct connect only (`index.md` §4).
- Authentication, anti-cheat, tamper-resistant identity.
- Shipping mod content to a client that lacks it.
- Multiple simultaneous sessions in one process.
- PvP, teams, or any session model other than co-op.
