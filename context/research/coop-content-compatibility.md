# Co-op Content Compatibility — Versioning Policy

> **Read this when:** deciding whether a content or script change breaks co-op, adding a new
> client-local simulation input, or considering networked content distribution.
> **Status:** design intent. The mechanism is specced in
> `plans/drafts/E15--session-lifecycle/`; nothing here is shipped.
> **Related:** [Networking](../lib/networking.md) · [Co-op Session and Lobby](./coop-session-lobby.md) · [Boot Sequence](../lib/boot_sequence.md) §8

---

## 1. The question a versioning policy has to answer

Not "what changed in the mod." The only question that matters is:

> **What does a client compute locally that the host will never correct?**

Everything outside that set is absorbed by server authority and cannot break a session no
matter how much it changes. The set is small, enumerable, and — importantly — mechanically
hashable, which is what lets compatibility be a property of content rather than a promise
by an author.

## 2. The three tiers

### Tier 1 — already covered; a version field would be redundant

| Surface | Covered by |
|---|---|
| Wire layout, message vocabulary | `PROTOCOL_ID` / `WIRE_VERSION`, transport gate |
| Mover authoring, mover collision | The static-kinematic fingerprint |

A code-level wire break cannot establish a connection at all. Neither needs a declared
version to catch it.

### Tier 2 — absorbed by server authority; divergence is harmless

The engine suppresses client-local simulation aggressively, and every suppression widens
this tier. Change any of these freely between builds; a client on the old one gets the
host's version.

- **Map-placed AI enemies and their placements.** A connected client does not spawn them.
  It materializes from host snapshots with mesh presentation only — never `Brain`, `Agent`,
  `Health`, or `Weapon` (`networking.md` §Phase boundaries).
- **Runtime enemy spawns** (`spawnFromSpawner`), including ones fired through the client's
  own reaction drain — suppressed by the `SpawnContext` runtime-spawn authority flag.
- **AI brain tuning** — `moveSpeed`, sight and leash distances, timers. Descriptor-owned,
  host-only. A client never runs a brain.
- **Trigger volumes.** Shared baked map data, but only the host evaluates touch/use overlap;
  the client sends a `use_pressed` bit and reconciles the consequences.
- **Trap-pool arming.** Host-only, load-time, consequences-only. The roll never crosses the
  wire and clients never re-run it.
- **Player start placement.** The host spawns pawns; clients receive them replicated.
- **Weapon damage, ammo, reserve, reload timing.** Host-authoritative, delivered as
  owner-private state-slot projections.
- **Particles, sprites, lights, fog volumes.** Client-local and off-wire by replicable-set
  policy.

### Tier 3 — the actual breaking surface

Nothing corrects these. This is the whole compatibility problem.

| # | Surface | Why it breaks | Lives in |
|---|---|---|---|
| 1 | **Static world collision geometry** | Client movement prediction runs against the local `CollisionWorld`; client-authoritative hit declaration casts against the world the client renders, and the host validates against *its* static geometry. Divergence means reconciliation fighting and false-rejected shots — silently. | `.prl` (`LevelWorld` vertices/indices) |
| 2 | **`PlayerMovementDescriptor` tuning** | Descriptor-authored and used by client prediction. Changed speed, jump, dash, or crouch diverges every tick. `view_feel` is the one exception — a render-only camera effect. | manifest `entities` |
| 3 | **Entity class identity** | Clients materialize remote entities by `entity_class`, matched against a descriptor's `canonical_name`. A renamed or removed name leaves the client unable to materialize what the host names. | manifest `entities` |
| 4 | **State-slot schema** | Clients apply replicated state-slot records against their local declaration. | manifest `store_declarations` |
| 5 | **World gravity set from a reaction** | Feeds local prediction; both peers run reactions. | manifest `reactions` / map KVP |

Tier 3 does not reduce to one mechanism. Item 1 and item 5's map half belong to the level
digest. Items 2 and 3 belong to the mod digest. Item 4 is already covered by a shipped schema
fingerprint and needs nothing new. Item 5's reaction half is covered by nothing — see
*Knowingly uncovered* in §3.

## 3. The policy

**Compatibility is decided by content, at the two moments the session gate already runs.**

| Stage | Compares | On mismatch |
|---|---|---|
| **Admission** | mod id | refuse and close |
| | mod version | nothing — display and diagnostics only |
| **Content parity** | mod compatibility digest, level identity, level content digest | hold, never close |

The split is by **mutability**, not by subject. Admission carries only what cannot change
for a live connection — the protocol constants, and the mod id, which is frozen at first
commit. Every content-derived value sits in parity, because every one of them can be
reinstalled under a live connection: the level pair at each install, the mod digest at each
staged reload. A mismatch on a parity value is therefore a *hold* with a diagnostic, never a
disconnect — it is a fact scheduled to become true again.

- The **mod compatibility digest** covers two things per entity type in the manifest's
  `entities` lane: the `canonical_name`, and the `PlayerMovementDescriptor` under `movement`
  — Tier 3 items 2 and 3. Nothing else on `EntityTypeDescriptor` is client-simulated.
  `default_weapon`, `weapon`, `health`, `ai`, and `behavior` are host-authoritative — Tier 2
  by §2, safe to change freely; `light`, `emitter`, and `mesh` are presentation. Hashing the
  lane wholesale would demote every peer on an enemy retune, which is the exact false refusal
  content-derived compatibility exists to prevent. Within the movement descriptor,
  `view_feel` is skipped as render-only; the other nine fields are prediction inputs and are
  hashed. A descriptor whose `canonical_name` is `None` is excluded outright: a client
  materializes remote entities by matching `entity_class` against `canonical_name`, so an
  unnamed descriptor cannot cross the wire and cannot diverge.
- The **level content digest** is the static-kinematic fingerprint widened to cover static
  world collision — Tier 3 item 1 — on the same rule that put mover collision there: a
  deterministic prediction input belongs in the parity hash.
- **State-slot parity is not the mod digest's.** A schema fingerprint over replicated state
  slots already ships: the replicated-slot schema hashes every replicated slot's dotted name,
  type, range, and scope under its own stream version, and both peers already compare it on
  every snapshot carrying state records. A second mechanism over `store_declarations` would
  duplicate it. It has one live defect: the schema is built lazily and cached for the process
  with no reset path, so it goes stale after a staged reload that adds a store namespace. Fix
  that where it lives; do not route around it with a digest.
- **Mod id still gates** because it is the namespace that makes a map catalog id resolvable
  on both peers.
- **Mod version never gates.** Exact equality blocks a friend on the previous build over a
  Tier 2 change no client ever simulates.

The recipe reaches nothing else in the manifest. Presentation lanes — `render`, `theme`,
`fonts`, `ui_trees`, `frontend` — are never reached, because hashing presentation would break
co-op on every cosmetic edit. `maps` is never reached, because a catalog divergence is a
recoverable named case.

### Knowingly uncovered

`reactions`, `crossings`, `events`, `trigger_events`, and `trigger_pools` are simulated, not
presentational, and no digest covers them. Tier 3 item 5 — world gravity set from a reaction
— lives in exactly this set. A mod whose reaction lane differs can pass both gates and
diverge on locally-simulated gravity.

This is a named gap, not an oversight, and the fix is not to widen the digest over those
lanes. That repeats the mistake the `entities` lane already demonstrates: it demotes peers
over changes no client simulates. Closing it properly means deciding which reaction effects
are client-local prediction inputs and hashing those. Until that decision is made the gap is
stated rather than omitted, because §6's rule is that silence about an uncovered simulated
input is the failure mode worth preventing.

### Why not an author-declared compatibility key

It moves the judgement to a human who will get it wrong silently, and the failure mode is
not a clean refusal — it is prediction fighting and false-rejected hits, the worst kind of
wrong. A digest cannot be got wrong.

### Why not a content hash over the whole mod

It changes on every hot reload, breaking the dev iteration loop, and it makes legitimate
client-side differences fatal. Scoping the digest to the simulated surface is what makes
content-derived compatibility usable.

Scoping is only half of it, though. The other half is that a divergence **holds** rather than
closes: a staged reload that moves the mod digest demotes affected peers to admitted with a
diagnostic, and they re-participate when the peers agree again. Both halves matter and
neither substitutes for the other. Mod **identity** is the one value frozen across reloads —
first-commit-wins — because admission is terminal and has no state to demote to.

### The digest domain is a denylist — at field granularity

The denylist works over **fields inside a named type**, never over whole manifest lanes.
Naming a lane and hashing all of it is what produces false refusals. Naming a type and hashing
all of its fields is what prevents silent omissions. So the rule has two halves and both are
load-bearing:

1. **The set of types the recipe reaches is named explicitly and kept deliberately small** —
   `EntityTypeDescriptor`, `PlayerMovementDescriptor`, and the structs beneath it. Widening
   that set is a decision, made once, in the open.
2. **Within each named type, every field is bound by exhaustive destructuring with no `..`
   rest pattern**, so a newly added field fails the build until someone classifies it as
   hashed or skipped.

An allowlist of field names is the mechanism that produced the static-collision omission in
the first place — a field added later by someone who never reads the recipe defaults to
unhashed, and no test catches a field you forgot. Under exhaustive destructuring the same
omission fails loud (a cosmetic edit demotes peers, visible immediately) instead of silent
(prediction fighting, traced back to nothing).

Three further requirements, all correctness rather than ergonomics:

- **Every map-valued field the recipe reaches hashes in key-sorted order.** A
  `std::collections::HashMap` under the default `RandomState` iterates in an order that
  differs *per process*, so two peers on byte-identical content would otherwise compute
  different digests. Descriptor data already carries such maps —
  `HealthDescriptor::zone_multipliers` is one — so the hazard is live the moment the type set
  widens.
- **Struct destructuring gives no exhaustiveness over enums.** An enum in the domain needs a
  `match` with no wildcard arm. Separate rule, separate enforcement; the destructuring rule
  does not imply it.
- **The mod digest carries its own epoch constant**, bumped whenever the recipe changes,
  mirroring the level digest's. Without it, a recipe change that alters the byte stream lets
  an old peer's digest accidentally match a new peer's.

## 4. How often is a change non-breaking?

For an ordinary authoring loop — moving enemy placements, retuning encounters, adjusting
lights, editing trigger volumes, iterating UI — **almost always**. Those are all Tier 2.

The breaking edits are geometry changes and shared-tuning changes. In a boomer-shooter map's
life, geometry work clusters early and encounter/tuning work runs constantly and dominates
late. So the non-breaking fraction *rises* over a map's life, and a declared-version policy
bites hardest exactly when friends are playtesting. That asymmetry is the practical argument
for content-derived compatibility, independent of the correctness one.

## 5. Why content distribution is not the answer

Sending a joining client the mod's scripts is tempting and does not solve the problem.

**Size is not the objection.** Measured against the dev mod: textures 291M, models 43M, maps
3.0M, **scripts 160K** — 0.05%. Scripts stay small by construction, not by discipline: the
typed command buffer is declarative and cannot iterate, so there are no algorithms or data
tables growing inside them. A total conversion multiplies art, not script mass.

**Three things are the objection:**

1. **It fixes the cheap third.** Script sync covers everything Tier 3 sources from the mod —
   items 2–4 and item 5's reaction half — and leaves item 1 untouched: the expensive one, in
   `.prl` bakes, with the worst failure mode. Making joins
   genuinely content-safe means shipping maps, which pulls textures and models: the 337M, not
   the 160K.
2. **Boot ordering inverts.** Mod init runs after `Session::build` constructs the net
   endpoint, and the VM drops after load. Receiving scripts at join means committing the
   entity registry, store declarations, UI trees, theme, and fonts *after* a connection
   exists — or re-running mod init mid-session, which is the `boot_sequence.md` §8
   hot-swap non-goal. The staged-reload path that exists is debug-only and re-commits
   `entities`/`store_declarations` only.
3. **Trust escalates.** Not "arbitrary code execution" — the scripting surface is narrow,
   declarative, and drops. But it is peer-controlled input to a C interpreter, with
   manifest-commit authority, in a project whose rule is no `unsafe` without approval. That
   is a different bar from "I trust my friends not to cheat at PvE."

If distribution ever lands, the ordering is: identity legible on the wire → digests covering
the real breaking surface → *then* distribution, out-of-band through a package path, never
over the reliable Control channel, which is a game channel and not a file transfer.

## 6. The rule to apply going forward

> **Any new input a client simulates against locally must join a digest in the same change
> that introduces it.**

Static world collision is the cautionary case: it was always a client-local prediction input
and was never hashed, so the gate has been passing maps it should have refused since
prediction shipped. The failure is silent, which is what makes the rule worth stating as a
rule rather than leaving to review.

A rule alone does not enforce anything, which is why the digest domains are denylists — the
compile error is what actually holds the line, and this paragraph is what explains why the
compile error is there. And a second rule, for the lane rather than the domain:

> **A compared value goes in admission only if a mismatch on it can never become a match.
> Everything else is parity, and parity holds rather than closes.**

Both rules exist because the same mistake is easy in two places: putting a value where it is
convenient rather than where its mutability says it belongs.
