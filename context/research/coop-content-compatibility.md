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
| 2 | **`PlayerMovementDescriptor` tuning** | Descriptor-authored and used by client prediction. Changed speed, jump, or crouch diverges every tick. | manifest `entities` |
| 3 | **Entity class identity** | Clients materialize remote entities by `entity_class`. A renamed or removed class leaves the client unable to materialize what the host names. | manifest `entities` |
| 4 | **State-slot schema** | Clients apply replicated state-slot records against their local declaration. | manifest `store_declarations` |
| 5 | **World gravity set from a reaction** | Feeds local prediction; both peers run reactions. | manifest / map KVP |

Tier 3 splits cleanly in two — items 1 and 5's map half live in the level, items 2–4 in the
mod — and that split is why the policy has two digests rather than one.

## 3. The policy

**Compatibility is decided by content, at the two moments the session gate already runs.**

| Stage | Compares | Gates on |
|---|---|---|
| **Admission** | mod id, mod compatibility digest | both |
| | mod version | nothing — display and diagnostics only |
| **Content parity** | level identity, level content digest | both |

- The **mod compatibility digest** covers `entities` and `store_declarations` — Tier 3
  items 2–4. It deliberately excludes `render`, `theme`, `fonts`, `ui_trees`, and
  `frontend`, because hashing presentation would break co-op on every cosmetic edit, and
  excludes `maps`, because a catalog divergence is a recoverable named case.
- The **level content digest** is the static-kinematic fingerprint widened to cover static
  world collision — Tier 3 item 1 — on the same rule that put mover collision there: a
  deterministic prediction input belongs in the parity hash.
- **Mod id still gates** because it is the namespace that makes a map catalog id resolvable
  on both peers.
- **Mod version never gates.** Exact equality blocks a friend on the previous build over a
  Tier 2 change no client ever simulates.

### Why not an author-declared compatibility key

It moves the judgement to a human who will get it wrong silently, and the failure mode is
not a clean refusal — it is prediction fighting and false-rejected hits, the worst kind of
wrong. A digest cannot be got wrong.

### Why not a content hash over the whole mod

It changes on every hot reload, breaking the dev iteration loop, and it makes legitimate
client-side differences fatal. Scoping the digest to the simulated surface is what makes
content-derived compatibility usable. Both digests are **first-commit-wins** across hot
reloads for the same reason; hot reload is debug-only, so this is a release guarantee and a
debug best-effort.

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

1. **It fixes the cheap third.** Script sync covers Tier 3 items 2–4 and leaves item 1
   untouched — the expensive one, in `.prl` bakes, with the worst failure mode. Making joins
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
