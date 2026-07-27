# Per-Player Currency (E16)

## Goal

Mod state has exactly one cardinality — global — so any mod-declared currency is team-shared in co-op. Give a store slot **per-owner cardinality**: an author declares whether a slot holds one value for the session or one value per player. That needs an owner identity the engine does not have, so this spec mints one — a **player seat** that survives level transitions, unlike the pawn and connection ids the engine keys per-player state by today. Owner-addressed writes reach a per-owner slot from an impact policy and from a reaction. The engine gains no currency, only the capacity to hold one per player.

Session-scoped. Saving a per-player value to disk and carrying it into someone else's session is `E16--per-player-persistence`, which sequences after this.

## Prerequisites

- **`E16--resource-grant-chokepoint`** — establishes source-addressed effect application: a planned command carries the token it addresses rather than assuming the dispatch target, and the bind guard becomes an expected-token checker. The owner-addressed `slot.add` here is the second consumer of both.
- **Epic 15 Phase 3.5** (shipped) — the owner-private replication scope, whose wire tracker already keys values by slot and owner, and which reserved the `ownerPrivate` declaration for mod stores "until a per-player authoring namespace exists." This is that namespace.
- **`E16--impact-policy-substrate`** (shipped) — the `slot.add` effect this gives a target token, and the evaluate-then-apply snapshot model its reads obey.

## Scope

### In scope

- **Player seat** — a session-stable per-player identity that survives level transitions, minted when a player enters the session and re-bound to that player's current pawn on each level install and to their connection on accept.
- **Per-owner slot cardinality** — a mod slot declares `perOwner: true`; its record holds one value per seat instead of one global value.
- **Owner-addressed writes** — a target token on the shipped `slot.add` so an impact policy can credit the damage source, plus a tag/activators-targeted reaction primitive so a trigger, crossing, or level-load reaction can credit a set of players.
- **Owner-addressed reads** — a policy reads a per-owner slot against an explicit owner token.
- **Publish paths** — host and single-player through the HUD slot publisher resolving the local seat; connected clients through the owner-private replication projection, which today serves one global value to every owner for a mod slot.
- **Reference per-player XP** in the dev mod, beside a shared counter, so the authoring choice is visible in one file.

### Out of scope

- **Persistence and the join seed** — `E16--per-player-persistence`. Both need decisions this spec should not smuggle: a connected client does not write the save file today (a shipped Phase 3.5 rule), and a client-to-host state write is a stated Phase 3.5 non-goal. Reversing either is deliberate work with its own Direction, and neither is needed for a per-owner currency to work within a session. Consequence, stated plainly: per-player XP does not survive a quit until that spec lands.
- **Cross-device or account identity.** A seat is session-scoped. A player who disconnects releases their seat and its values; a rejoin is a new seat at declared defaults.
- **Source-addressed per-entity state.** Nothing here writes another entity's per-entity state, so `scripting.md` §11's same-entity seam is untouched and `E10--enemy-aggro-model`'s cut rationale stays intact.
- **Non-player owners.** Seats belong to players. An enemy's own counter stays per-entity state, which already ships.
- **Per-key / per-category slots.** `xpByWeapon.shotgun` needs a per-key mechanism that does not exist.
- **Client-authored writes and prediction.** The host is authoritative; a client receives values.
- **Engine-owned resources.** Health and ammo belong to `E16--resource-grant-chokepoint`.
- **Exact integer currencies past 2^24.** Slots are `f32`; a currency beyond ~16.7M silently rounds. `scripting.md` §5 already notes exact integer slots would need their own state-store and replication contract.

## Direction

**Problem.** A mod currency can only be a global store slot, so in co-op every player shares one pot. The cause is store cardinality — which is why the fix belongs to the store rather than the damage path. Underneath it sits a second cause this spec must also fix: the engine has no player identity that outlives a level. Per-player *within-level* state (trigger occupancy, use edges, alive players) is rebuilt each level, so a pawn id has always been enough; a value that outlives the level is the first thing that needs more.

**Prior commitments.**
- *The spelling was reserved for this.* Phase 3.5 allowed mod slots `network: "shared"` and rejected `network: "ownerPrivate"` in code with a named diagnostic, "until a per-player authoring namespace exists." This supplies the namespace and unlocks the rejection.
- *The wire already has this cardinality.* The replication tracker keys values by slot and owner, and the owner-private resolver dispatches per-owner projections ahead of a global fall-through. Per-owner mod slots extend a shipped shape and replace a hardcoded name-dispatch chain with a declaration-driven one.
- *The store outlives level unload* (`scripting.md` §5). Honored by the seat: a per-owner value is keyed by something that outlives the level, so a per-owner slot obeys the same durability contract a global one already does. **The previous draft of this spec violated this**, keying by a pawn id that level unload invalidates — a per-player value would have reset at every level change, including a single-player restart, while global slots survived.
- *Currencies are mod-owned* (`combat-events.md` §2). The engine gains cardinality, not XP.
- *Fan-out needs tag targeting.* `E16--resource-grant-chokepoint` established that the IR has no iteration, so reaching several recipients in one fire is only expressible through tag or activators targeting. The reaction write path is not optional here either.
- **Divergence, named:** `scripting.md` §12 states store slots are ambient and ambient refs do not enlarge a reaction's dispatch scope. An owner-addressed read binds an ambient store slot against an ephemeral dispatch token, so a per-owner read *does* enlarge the scope and cannot appear in a sourceless reaction. Deliberate: per-owner values have no meaning without an owner, and the alternative — an implicit owner — is a wrong-owner bug. Global slot reads are unaffected. `scripting.md` §12 wants this recorded at promotion.

**Placement.** Cardinality is a property of the slot, so it lives on the slot. The rejected alternative — backing a per-player slot with a field on the player's pawn — put cardinality in the impact layer, which made the currency non-persisting, readonly, and awardable only by dealing damage. All three fell out of the placement. The seat lives in the entities floor crate rather than the binary, because the slot table it keys is a floor-crate structure; the existing player id is `pub(crate)` in the binary and cannot be named from there.

**Alternatives rejected.**
- *Fusing cardinality into `network: "ownerPrivate"`* — one key meaning both "one value per player" and "replicated privately." Tempting because Phase 3.5 reserved that spelling and because the two coincide for the motivating case. Rejected: the axes are orthogonal, and fusing them makes a per-player slot the HUD never shows — host-side bookkeeping — inexpressible, along with any future shared-but-privately-delivered value. Phase 3.5 reserved a *spelling* for when a namespace existed; it did not decide the two concepts were one. Cardinality gets its own declaration and `network` stays purely about replication.
- *A slot as a view of a per-entity state field on the owning pawn* — an earlier draft, reworked after review. The value could not persist (components die with the level), could be earned only by dealing damage (per-entity state has one write site, inside an impact policy), and fused cardinality with backing.
- *Host issues reward deltas; the client accumulates its own total.* Better trust story, but reward policy evaluates host-side, so a policy reading a currency ("double past level 10") could not see it.
- *Making the seat its own foundational spec.* Every existing per-player consumer — trigger occupancy, use edges, alive players, canonical pawns — is within-level and rebuilt each level, so the seat has exactly one consumer needing durability: this spec. A standalone spec would ship a mechanism with no observable outcome, and the epic's own precedent is the impact substrate bundling the keystone with the dispatch rather than stacking them.

## Decisions

- **A seat is minted on entry and released on exit.** The local player takes a seat at boot; each accepted client takes the next. A disconnect releases the seat and drops its values. A rejoining player is a new seat at declared defaults — honest for a session-scoped mechanism, and the case `E16--per-player-persistence` addresses by restoring from that player's own save.
- **The seat is the durable key; the existing player id stays the within-level address.** The seat registry maps a seat to its current pawn and, for a remote player, to its connection. Re-binding happens on level install (new pawn) and on accept. The engine's existing per-player state keeps using the id it uses today — this spec does not rewrite those call sites.
- **Cardinality and replication are separate declarations.** `perOwner: true` selects per-seat storage; `network` continues to mean only how a value reaches clients. `network: "ownerPrivate"` requires `perOwner: true` — a single global value fanned privately to each owner is meaningless and is a load error. `perOwner: true` alone is legal and means host-local per-player state.
- **Writes and reads are owner-addressed.** A per-owner slot is written and read only through a token-addressed form. **A bare access to a per-owner slot is a load error naming the slot**, rather than resolving to a default owner — an implicit owner is a wrong-owner bug that surfaces as one player's rewards landing on another. A token on a global slot is equally an error: declaration and access must agree.
- **`slot.add` gains a target token; its untargeted form is unchanged.** An absent token means the global slot exactly as today, so every shipped policy keeps working. This modifies a shipped effect arm rather than adding one — the widest blast radius here, and why the untargeted path must stay behaviorally identical.
- **A recipient with no seat is skipped with a warning.** An owner-addressed write resolving to an enemy, a prop, or a player mid-disconnect writes nothing and does not abort sibling effects.
- **Reads observe the per-fire frozen snapshot**, like every other store read in a policy, so a gate never sees a write from its own fire.
- **No wire version change.** Per-owner values ride the existing owner-private replication path, and the replicated-schema fingerprint is content-derived, so adding slots costs no constant bump. The join seed — which would cost both the app-protocol and wire constants — is the other spec.
- **The UI is untouched.** Widgets bind by slot name and receive the local player's value; resolution happens in the publisher and the replication projection.

## Acceptance criteria

- [ ] A slot declared per-owner holds independent values for two players: crediting one leaves the other unchanged, in the same session.
- [ ] **A per-owner value survives a level transition** — after a level change or a single-player restart, a per-owner slot reads what it held before, exactly as a global slot does.
- [ ] An impact policy credits the damage source's own value; a reaction credits every activator that entered a trigger volume, and a tag-targeted reaction credits every matching player.
- [ ] A currency is awardable without dealing damage — a trigger volume, a crossing, and a level-load reaction each credit a per-owner slot.
- [ ] A bare read or write of a per-owner slot fails at load with a diagnostic naming the slot; so does an owner-addressed access to a global slot, and so does `network: "ownerPrivate"` declared without per-owner cardinality. Other declarations in the same manifest still load.
- [ ] A slot declared per-owner without a replication scope holds independent per-player values host-side and is not replicated.
- [ ] An untargeted `slot.add` on a global slot behaves exactly as before — the shipped dev-mod policies that use it are unchanged.
- [ ] An owner-addressed write resolving to a non-player entity writes nothing, warns, and leaves sibling effects in the same fire applying normally.
- [ ] A policy whose reward amount reads the same per-owner slot it credits observes the pre-fire value, not its own write — two hits in one tick accrue both increments.
- [ ] In co-op, each client's HUD shows only its own value; a second client's value never leaks across, including for a late joiner.
- [ ] A disconnecting player releases its seat; a subsequent joiner does not inherit its values.
- [ ] Reference walkthrough: the dev mod awards per-player XP on a kill and increments a shared counter in the same policy; across two clients the XP readouts diverge while the shared counter agrees, and the only difference in the script is which slot each writes.

## Tasks

### Task 1: Player seat

Add a session-stable per-player identity in the entities floor crate, where the slot table that will key by it lives — the binary's existing player id is crate-private there and cannot be named from the floor. A seat is minted when a player enters the session (the local player at boot, each accepted client on accept) and released when they leave. The registry holds, per seat, the player's current pawn and — for a remote player — their connection id, with reverse lookups both ways: an entity to its seat (what an owner-addressed write needs, since effects address pawns) and a seat to its connection (what the replication projection needs, since the owner-private tracker keys by client). **Re-bind rather than re-mint on a level install:** level unload despawns every entity and bumps generations so old ids never revalidate, so the seat's pawn binding must be refreshed when the next level's pawns spawn — this is the whole reason the seat exists, and a per-owner value keyed by a pawn id would reset at every level change while global slots survived. Single-player has exactly one seat and no path branches on player count. Do not rewrite the existing within-level player-id call sites; the seat is a parallel durable key, not a replacement addressing scheme.

### Task 2: Per-owner slot cardinality

Give the slot table per-seat storage and the declaration that selects it. A mod slot may declare per-owner cardinality; its record then holds a value per seat instead of one global value, with the declared default serving any seat not yet written. Thread the declaration through the slot schema, the SDK store-slot type and its generated typedef, and both descriptor parsers, so a Luau and a TypeScript mod declare it identically. Keep cardinality and replication as separate declarations: unlock the shipped rejection of `ownerPrivate` for mod stores (`replication_scope_for` in the store bridge currently returns an error for it) but require per-owner cardinality alongside it, since a single global value delivered privately to each owner is meaningless. Add the load-time rejections that keep declaration and access in agreement — a per-owner slot accessed without an owner token, a global slot accessed with one, and `ownerPrivate` without per-owner cardinality — reported where the shipped slot-declaration validation already reports malformed schemas, with the rest of the manifest still loading. The global path stays untouched: a slot without the declaration stores and reads exactly as today.

### Task 3: Owner-addressed writes

Make a per-owner slot writable from both paths, mirroring the dual `E16--resource-grant-chokepoint` establishes for grant. **Impact policy:** the shipped `slot.add` effect rejects any target today and lowers to a self-referential add on a global slot; give it an optional target token, reusing the expected-token checker the grant spec generalizes rather than adding a third guard, and route a targeted add to the addressed player's seat via Task 1's entity-to-seat lookup. An absent token keeps today's global behavior byte-for-byte. **Reaction:** register a primitive crediting a named per-owner slot for every target, accepting the activators token or a tag, so a trigger volume, a crossing, or a level-load reaction can award a currency — the only path reaching several players in one fire, since the IR has no iteration. Both paths skip a recipient with no seat, warn, and continue. Export the SDK builders for both, mirroring the shipped damage builder's activators-or-tag dual.

### Task 4: Owner-addressed reads and publish paths

Give a policy a way to read a per-owner value and the HUD a way to see it. **Read:** an owner-addressed read form binding a per-owner slot against an explicit owner token, resolved per fire through the same entity-to-seat lookup the write uses; a bare read is Task 2's load error. Reads come off the per-fire frozen snapshot the evaluator already applies to store reads. **Publish, host and single-player:** the HUD slot publisher republishes player slots each frame from local state — resolve per-owner slots against the local seat there, following the existing no-value skip that leaves a slot at its previous value rather than resetting it. **Publish, connected clients:** the owner-private source resolver dispatches named projections ahead of a global fall-through, and a mod slot currently falls through to one global value served to every owner; add the per-seat lookup ahead of that fall-through, mapping each owner's connection to its seat. That file's non-test body is past the size guidance — add the lookup as a sibling helper beside the existing projections and do not restructure. Test cross-owner isolation explicitly, including a late joiner and a disconnect.

### Task 5: Reference per-player XP

Ship the reference economy in the dev mod. Declare a per-owner XP slot and a shared session counter in the same store, and extend the reward policy the grant spec adds to `content/dev/scripts/combat-lifecycle.ts` to credit both on a kill — the per-owner slot addressed to the damage source, the shared one untargeted. Add the XP readout to the dev HUD (`content/dev/scripts/hud.ts`). Comment the pair so the authoring choice is legible: same reward, same policy, and the declaration decides whether it is one pot or one per player. Update the combat demo README walkthrough with the two-client divergence, and note that XP is session-scoped until the persistence spec lands.

## Sequencing

**Phase 1 (sequential):** Task 1 — the seat everything keys by.
**Phase 2 (sequential):** Task 2 — per-seat storage over Task 1's identity.
**Phase 3 (concurrent):** Task 3, Task 4 — writes and reads/publish are independent over Task 2 and touch disjoint files.
**Phase 4 (sequential):** Task 5 — consumes all of it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A per-owner value outlives a level transition | Task 1 (seat re-binds to the new pawn) | Level unload invalidates every entity id — re-binding is the guard, and a missed re-bind resets values silently | AC 2 |
| One owner's value never reaches another owner | Task 2 (per-seat storage), Task 4 (lookup ordered before the global fall-through) | The resolver's fall-through currently serves one global value to every owner — ordering is the guard | AC 1, 10 |
| A per-owner value is written only through an owner-addressed path | Task 2 (load-time rejection), Task 3 (both write paths) | Any bare-write path added later re-opens the wrong-owner bug | AC 5 |
| The global slot path is behaviorally unchanged | Task 2, Task 3 (untargeted `slot.add` untouched) | Shared with every shipped policy that writes a store slot | AC 7 |
| Cardinality and replication stay independent | Task 2 (separate declarations, one cross-check) | A later shortcut that infers one from the other re-fuses the axes | AC 5, 6 |
| A gate never observes a write from its own fire | Task 4 (reads from the frozen snapshot) | Shared with the evaluator's evaluate-then-apply model | AC 9 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| player seat | floor-crate seat registry | not replicated — seats map to existing connection ids | — (not author-facing) | — |
| per-owner cardinality | per-seat slot storage | slot declaration key | `perOwner: true` | same |
| replication scope | existing owner-private scope | existing scope tag | `network: "ownerPrivate"` | same |
| owner-addressed add (impact) | `slot.add` effect with a target token | effect args plus the addressed token | `slot.of(impact.source).add(delta)` | `slot:of(impact.source):add(delta)` |
| owner-addressed add (reaction) | reaction primitive | primitive name, `target?: "@activators"` or tag, args carry slot and delta | `addSlot(target, slot, delta)` | same |
| owner-addressed read | seat-bound store read | existing store input leaf, seat resolved per fire | `slot.of(impact.source)` | `slot:of(impact.source)` |

## Script syntax examples

```ts
// Proposed design — every currency here is declared by the mod. The engine
// ships cardinality, not XP.
const { state: progression } = defineStore("progression", {
  // One per player, replicated to its owner. Session-scoped until the
  // persistence spec lands.
  xp:        { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  // One per player, host-side only — the HUD never shows it.
  killStreak:{ type: "number", default: 0, perOwner: true },
  // One for the session, shared by everyone.
  teamKills: { type: "number", default: 0 },
});

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const bonus = impact.target.healthAfter.le(-40).select(50, 25);
  return [
    { when: killed, do: [
        progression.xp.of(impact.source).add(bonus),   // per player
        progression.teamKills.add(1),                   // shared, unchanged from today
    ]},
  ];
});

// Awardable without dealing damage — the objective volume pays everyone in it.
onTriggerEvent({ tag: "objective" }, "enter", [
  defineReaction((on: TriggerEventParams) => seq([addSlot(on.activators, progression.xp, 100)])),
]);
```

## Open questions

- **Rejoin within a session.** A disconnect releases the seat and its values, so a player who drops and returns starts at defaults. `E16--per-player-persistence` restores them from that player's own save, which makes the gap smaller but not zero for a mid-session blip. Holding released seats for a grace period is possible but needs a stable rejoin key, which does not exist.
- **`scripting.md` §12 update at promotion.** The ambient-refs-do-not-enlarge-scope property gains an exception for owner-addressed reads.
- **Per-weapon proficiency.** Expressible only as one declared slot per weapon class; the per-key mechanism `combat-events.md` sketches remains unbuilt and demand-gated.
