# Combat Movement & Tactics — syntax mockups

Illustrative author-facing syntax for the design line in
`../combat-movement-and-tactics-api-design.md`. **These files do not compile** and are not content.
They exist to make ergonomic tradeoffs concrete before a spec is drafted.

They are built on the **real** current authoring surface (`defineEntity` / `components.behavior`,
`brain.*` guard leaves, `runtime.*` builders, `move`/`offense` layers) so the only unfamiliar parts
are the deliberate proposals.

## Legend

- `// TODAY` — real current syntax, exists now.
- `// PROPOSED` — surface that does not exist yet; the point of discussion.
- `// Option A` / `// Option B` — a live fork; two concrete spellings to compare, not both adopted.
- `// LOWERS TO` — what a proposed sugar compiles to in today's IR.

## Files and the fork each probes

| File | Behavior | Fork it surfaces |
|---|---|---|
| `01-strafe-kite.ts` | Standoff that widens when hurt; orbit while shooting | **IR-computed motion params** — the brain scope must resolve an *output* (today it resolves none). Also verb-collapse vs. verb-add. |
| `02-tactics-sequence.ts` | "fire → strafe 2m → fire"; "fire ×3 → seek cover" | **Sequence sugar** vs. today's hand-rolled nested graph; where the **escape hatch** lives (outer wildcard, not a new interrupt). Needs a `displacementInActivity` fact. |
| `03-read-the-room.ts` | Pick a tactic from a repertoire, prune inapplicable, "sometimes" | **Selecting among *sequences*** is transition-routing, not the leaf-only selector layer. Stochastic choice = one new `brain.roll` fact. |
| `04-companion-follow.ts` | Allied brain that follows the player and takes commands | **Dynamic friendly anchor** (anchors are fixed/target-only today) and the net-new **command channel** input. |

## Runtime note

TypeScript is primary. Luau parity is 1:1 through the same descriptors; only the guard idiom differs
(`brain.x:le(y)` colon-calls; `["and"]`/`["or"]`/`["not"]` bracket-index for the reserved words).
The proposals here are all plain descriptor data, so they carry to Luau unchanged.
