# Per-Player Currency (E16)

## Goal

Mod state has exactly one cardinality — global — so any mod-declared currency is team-shared in co-op. Give a store slot **per-owner cardinality**: an author declares whether a slot holds one value for the session or one value per player, the same way they already declare its type and whether it persists. Owner-addressed writes reach it from an impact policy and from a reaction; each device persists its own player's values; a joining client seeds the session from what it saved. The engine gains no currency — only the capacity to hold one per player.

## Prerequisites

- **`E16--resource-grant-chokepoint`** — establishes source-addressed effect application: a planned command carries the token it addresses rather than assuming the dispatch target, and the bind guard becomes an expected-token checker. The owner-addressed `slot.add` here is the second consumer of both.
- **Epic 15 Phase 3.5** (shipped) — the owner-private replication scope, whose wire tracker already keys values by `(slot, owner)`, and which reserved the `ownerPrivate` declaration spelling for mod stores "until a per-player authoring namespace exists." This is that namespace.
- **`E16--impact-policy-substrate`** (shipped) — the `slot.add` effect this gives a target token, and the evaluate-then-apply model its writes obey.

## Scope

### In scope

- **Per-owner slot cardinality.** A mod slot declares `network: "ownerPrivate"`; its record holds one value per owner instead of one global value.
- **Owner-addressed writes.** The shipped `slot.add` effect gains a target token so an impact policy can credit the damage source, plus a tag/activators-targeted reaction primitive so a trigger, crossing, or level-load reaction can credit a set of players.
- **Owner-addressed reads.** A policy reads a per-owner slot against an explicit owner token.
- **Publish paths.** Host and single-player through the HUD slot publisher resolving the local owner; connected clients through the owner-private replication projection.
- **Device-local persistence.** Each device saves its own player's per-owner values, alongside its shared persisted slots.
- **Schema-driven join seed.** A joining client sends its saved values for persisted owner-private slots; the host validates each against the declared schema and seeds that owner.
- **Reference per-player XP** in the dev mod, with a HUD readout, sitting beside a shared counter so the choice is visible in one file.

### Out of scope

- **Source-addressed per-entity state.** This spec deliberately does not settle `scripting.md` §11's same-entity write seam — nothing here writes another entity's per-entity state. `E10--enemy-aggro-model` (in-progress) cut a feature pending that seam and warned against picking it without a consumer; its rationale stays intact and the choice stays open.
- **Integrity verification of the join seed.** The host trusts a guest's seeded values. The payload is structured so a later integrity field is an addition, not a reshape.
- **Cross-device or account identity.** A save file belongs to whoever owns the device; there is no profile, account, or cloud identity, and this spec invents none.
- **Non-player currencies.** Owners are players. An enemy's own counter stays per-entity state, which already ships.
- **Per-key / per-category slots.** `xpByWeapon.shotgun` needs a per-key mechanism that does not exist; a per-weapon bucket is expressible only as one declared slot per class.
- **Client-authored writes and prediction.** The host is authoritative during a session; a client receives values and saves them.
- **Engine-owned resources.** Health and ammo belong to `E16--resource-grant-chokepoint`.

## Direction

**Problem.** A mod currency can only be a global store slot, so in co-op every player shares one pot. The cause is store cardinality, not anything about combat — which is why the fix belongs to the store and not to the damage path.

**Prior commitments.**
- *The spelling was reserved for this.* `M15--p35-state-slot-replication` allowed mod slots `network: "shared"` and explicitly rejected `network: "ownerPrivate"` for mod stores "until a per-player authoring namespace exists," while pinning the internal enum names. This spec supplies that namespace and adopts the reserved spelling rather than inventing a second one.
- *The wire already has this cardinality.* The replication tracker keys values by slot and owner, and the owner-private source resolver already dispatches per-owner projections ahead of a global fall-through. Per-owner mod slots extend a shape that shipped, and replace a hardcoded name-dispatch chain with a declaration-driven one.
- *Currencies are mod-owned.* `combat-events.md` §2. Honored exactly: the engine adds no currency, only cardinality.
- *`combat-events.md` §5's `persist: true` on an XP slot.* Honored — a per-owner slot persists like any other, which the earlier draft of this spec could not deliver.
- *Fan-out needs tag targeting.* `E16--resource-grant-chokepoint` established that the IR has no iteration, so reaching several recipients in one fire is only expressible through tag or activators targeting. The same reasoning applies to currency, so the reaction write path is not optional here either.

**Placement.** Cardinality is a property of the slot, so it lives on the slot. The rejected alternative — backing a per-player slot with a field on the player's pawn — put cardinality in the impact layer, which made the currency non-persisting (components do not persist), readonly (its authority lived elsewhere), and awardable only by dealing damage (per-entity state has one write site, inside an impact policy). All three fell out of the placement, not the feature.

**Alternatives rejected.**
- *A slot as a view of a per-entity state field on the owning pawn* — the previous draft of this spec, and the reason it was reworked. `/validate-plan` returned **under-scoped**, and the diagnosis held up: fusing cardinality with backing meant a persisted per-player slot would need a second declaration spelling; the value could not persist at all, since per-entity state dies with the level; and because per-entity state is writable only from an impact policy, a currency could be earned only by dealing damage — no trigger volume, no crossing, no level-load seed. It also proposed overturning a `scripting.md` §11 invariant that an in-progress sibling plan is actively building on.
- *Host issues reward deltas; the client accumulates its own total.* Cleanest trust story — the host never holds a player's lifetime numbers, so there is nothing to falsify server-side. Rejected because reward policy runs host-side: a mod writing "double XP past level 10" needs the total to evaluate the rule, and a client-held total is unreadable from the policy. Expressiveness beats a trust improvement that friends-and-community play does not need.
- *A durable account or profile identity, so progression follows a player across devices.* Rejected as a product decision this spec should not make, and unnecessary: with device-local saves the file itself is the identity, and single-player — the dominant case — has exactly one owner.

## Decisions

- **Cardinality is declared with the reserved spelling.** `network: "ownerPrivate"` on a mod slot, no second key. A slot is global or per-owner; nothing else changes about how it is declared, typed, ranged, or persisted.
- **Owner identity is the existing session-scoped player id** (a local pawn or a remote client). It does not need to be durable, because persistence is device-local — the save file identifies its owner by belonging to that device. Single-player has exactly one owner, so no path branches on player count.
- **Writes are owner-addressed; reads are owner-addressed.** A per-owner slot is written only through a token-addressed path and read only through an explicit owner token. **A bare read or write of a per-owner slot is a load error naming the slot**, rather than silently resolving to some default owner — an implicit owner is a wrong-owner bug that would surface as one player's rewards landing on another. Conversely a token on a global slot is a load error too: the two declarations and the two access forms must agree.
- **`slot.add` gains a target token; its untargeted form is unchanged.** An absent token means the global slot, exactly as today, so every shipped policy keeps working untouched. This modifies a shipped effect arm rather than adding one — the wider blast radius in this spec, and the reason the untargeted path must stay byte-identical in behavior.
- **A non-player recipient is skipped with a warning.** Owners are players; an owner-addressed write that resolves to an enemy or a prop writes nothing and does not abort sibling effects.
- **Persistence is device-local and needs no format change.** Each device saves its own owner's values under the slot's declared name, so the save document stays a flat name-to-value map and no version bump is required. A save carries exactly one owner's values, and restoring applies them to the local owner.
- **The join seed is schema-driven, not progression-shaped.** At join a client sends its saved values for slots the session's schema marks persisted *and* owner-private. The host validates each against the declared type and range before seeding, ignores names the session's mods do not declare, and warns. Nothing in the message knows what a currency is — a mod declaring persisted per-player faction standing or unlocked-weapon flags gets the same path with no new work. Costs one wire version bump on the existing handshake.
- **The host trusts the seed.** A guest can hand over any value its save file contains. Acceptable for community and friend-group play, and the honest threat model is a player editing their own save, not a live-service economy. The payload is a structured record so a later integrity check adds a field rather than reshaping the message.
- **A guest that seeds nothing starts at the declared defaults**, and its values persist to its own device on exit like any other player's.
- **The UI is untouched.** Widgets bind by slot name and receive the local owner's value; the resolution happens in the publisher and the replication projection, not in the UI layer.

## Acceptance criteria

- [ ] A slot declared per-owner holds independent values for two players: crediting one leaves the other unchanged, in the same session.
- [ ] An impact policy credits the damage source's own value; a reaction credits every activator that entered a trigger volume, and a tag-targeted reaction credits every matching player.
- [ ] A currency is awardable without dealing damage — a trigger volume, a crossing, and a level-load reaction each credit a per-owner slot.
- [ ] A bare read or write of a per-owner slot fails at load with a diagnostic naming the slot; so does an owner-addressed access to a global slot. Other declarations in the same manifest still load.
- [ ] An untargeted `slot.add` on a global slot behaves exactly as before — the shipped dev-mod policies that use it are unchanged in behavior.
- [ ] An owner-addressed write resolving to a non-player entity writes nothing, warns, and leaves sibling effects in the same fire applying normally.
- [ ] A policy whose reward amount reads the same per-owner slot it credits observes the pre-fire value, not its own write — two hits in one tick accrue both increments without either reading the other's result.
- [ ] In single-player, a per-owner value survives a clean exit and restore, and the HUD reads it back.
- [ ] In co-op, each client's HUD shows only its own value; a second client's value never leaks across, including for a late joiner.
- [ ] A joining client's saved values seed its own owner and appear on its HUD; a guest with no save starts at the declared default.
- [ ] A seeded value that violates the declared type or range is rejected and the slot falls back to its default, with a warning naming the slot; a seeded name the session does not declare is ignored with a warning.
- [ ] Each device's save holds only its own player's values — a host's save does not accumulate its guests'.
- [ ] Reference walkthrough: the dev mod awards per-player XP on a kill and increments a shared counter in the same policy; the two readouts diverge across two players, and the only difference in the script is which slot each writes.

## Tasks

### Task 1: Per-owner slot cardinality

Give the slot table per-owner storage and the declaration that selects it. A mod slot may declare the reserved owner-private replication scope; its record then holds a value per owner keyed by the engine's existing session player identity (a local pawn or a remote client id) instead of one global value, with the declared default serving any owner not yet written. Thread the declaration through the slot schema, the SDK store-slot type and its generated typedef, and both descriptor parsers, so a Luau and a TypeScript mod declare it identically. Add the load-time rejections that keep the declaration and its access forms in agreement: an owner-private slot accessed without an owner token, and a global slot accessed with one, are both load errors naming the slot, surfaced where the shipped slot-declaration validation already reports malformed schemas — the rest of the manifest still loads. Keep the global path untouched: a slot without the declaration stores and reads exactly as today, since every shipped policy depends on it.

### Task 2: Owner-addressed writes

Make a per-owner slot writable from both paths, mirroring the dual `E16--resource-grant-chokepoint` established for grant. **Impact policy:** the shipped `slot.add` effect currently rejects any target and lowers to a self-referential add on a global slot; give it an optional target token, reusing the expected-token checker the grant spec generalizes rather than adding a third guard, and route a targeted add to the addressed owner's value. An absent token keeps today's global behavior byte-for-byte. **Reaction:** register a primitive that credits a named per-owner slot for every target, accepting the activators token or a tag, so a trigger volume, a crossing, or a level-load reaction can award a currency — this is the only path that reaches several players in one fire, since the IR has no iteration. Both paths skip a non-player recipient with a warning and continue. Export the SDK builders for both, mirroring the shipped damage builder's activators-or-tag dual.

### Task 3: Owner-addressed reads and publish paths

Give a policy a way to read a per-owner value and the HUD a way to see it. **Read:** an owner-addressed read form binding a per-owner slot against an explicit owner token, resolved per fire from the addressed owner exactly as the write is; a bare read of such a slot is the load error from Task 1. Reads observe the same per-fire frozen snapshot discipline the evaluator already applies to store reads, so a gate never sees a write from its own fire. **Publish, host and single-player:** the HUD slot publisher republishes player slots each frame from local state — resolve per-owner slots against the local owner there, following the existing no-value skip that leaves a slot at its previous value rather than resetting it. **Publish, connected clients:** the owner-private source resolver dispatches named projections ahead of a global fall-through; add the per-owner lookup ahead of that fall-through, so a per-owner slot never serves one value to every owner. That file's non-test body is past the size guidance — add the lookup as a sibling helper beside the existing projections and do not restructure. Test cross-owner isolation explicitly, including a late joiner.

### Task 4: Device-local persistence and join seed

Persist per-owner values and carry them into a session. **Persistence:** the save document is a flat map of slot name to value and stays that way — each device saves the local owner's value for a persisted per-owner slot under the slot's own name, so no format change and no version bump. Restoring applies saved values to the local owner. The collection and overlay paths already filter by persisted-mod-slot; extend them to resolve a per-owner slot against the local owner rather than skipping it. **Join seed:** add a client-to-host join-time message carrying the client's saved values for slots the session schema marks persisted and owner-private, as a structured record with room for a later integrity field. The host validates each entry against the declared schema — type and range, the same rules the overlay path applies to a save file — seeds the owner's value on success, falls back to the declared default with a warning on failure, and ignores with a warning any name its mods do not declare. Then normal replication carries the seeded value back to that client. This costs one wire version bump on the existing two-gate handshake. Nothing in the message, its validation, or its naming refers to currency or progression: any persisted owner-private slot rides it.

### Task 5: Reference per-player XP

Ship the reference economy in the dev mod. Declare a per-owner persisted XP slot and a shared session counter in the same store, and extend the reward policy the grant spec adds to `content/dev/scripts/combat-lifecycle.ts` to credit both on a kill — the per-owner slot addressed to the damage source, the shared one untargeted. Add the XP readout to the dev HUD (`content/dev/scripts/hud.ts`). Comment the pair to make the authoring choice legible: same reward, same policy, and the declaration is what decides whether it is one pot or one per player. Update the combat demo README walkthrough with both the single-player save-and-restore path and the two-client divergence.

## Sequencing

**Phase 1 (sequential):** Task 1 — the storage and declaration everything else binds against.
**Phase 2 (concurrent):** Task 2, Task 3 — writes and reads/publish are independent over Task 1 and touch disjoint files.
**Phase 3 (sequential):** Task 4 — persistence and the seed consume the schema from Task 1 and the replication path from Task 3.
**Phase 4 (sequential):** Task 5 — consumes all of it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| One owner's value never reaches another owner | Task 1 (per-owner storage), Task 3 (lookup ordered before the global fall-through) | The resolver's fall-through is the standing threat — ordering is the guard | AC 1, 8 |
| A per-owner value is written only through an owner-addressed path | Task 1 (load-time rejection), Task 2 (both write paths) | Any bare-write path added later re-opens the wrong-owner bug | AC 4 |
| The global slot path is behaviorally unchanged | Task 1, Task 2 (untargeted `slot.add` untouched) | Shared with every shipped policy that writes a store slot | AC 5 |
| A save holds exactly one owner's values | Task 4 | A host accumulating guests' values would leak progression between players | AC 11 |
| A seeded value is schema-valid before it is trusted as state | Task 4 | The seed is the one client-to-host state path; skipping validation admits malformed values the slot's own rules would reject | AC 10 |
| A gate never observes a write from its own fire | Task 3 (reads from the frozen snapshot) | Shared with the evaluator's evaluate-then-apply model | AC 7 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| per-owner cardinality | existing owner-private replication scope | existing scope tag | `network: "ownerPrivate"` | same |
| owner-addressed add (impact) | `slot.add` effect with a target token | effect args plus the addressed token | `slot.of(impact.source).add(delta)` | `slot:of(impact.source):add(delta)` |
| owner-addressed add (reaction) | reaction primitive | primitive name, `target?: "@activators"` or tag, args carry slot and delta | `addSlot(target, slot, delta)` | same |
| owner-addressed read | owner-token-bound store read | existing store input leaf, owner resolved per fire | `slot.of(impact.source)` | `slot:of(impact.source)` |
| join seed | join-time client message | structured per-slot record, room for a later integrity field | — (not author-facing) | — |

## Script syntax examples

```ts
// Proposed design — every currency here is declared by the mod. The engine
// ships cardinality, not XP.
const { state: progression } = defineStore("progression", {
  // One per player, saved to that player's own device.
  xp:        { type: "number", default: 0, persist: true, network: "ownerPrivate" },
  // One for the session, shared by everyone.
  teamKills: { type: "number", default: 0 },
});

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const bonus = impact.target.healthAfter.le(-40).select(50, 25);
  return [
    { when: killed, do: [
        // Per player: addressed to whoever landed the kill.
        progression.xp.of(impact.source).add(bonus),
        // Shared: untargeted, exactly as it works today.
        progression.teamKills.add(1),
    ]},
  ];
});

// Awardable without dealing damage — the objective volume pays everyone in it.
onTriggerEvent({ tag: "objective" }, "enter", [
  defineReaction((on: TriggerEventParams) => seq([addSlot(on.activators, progression.xp, 100)])),
]);
```

## Open questions

- **Seed integrity (owner call, deferred by agreement).** The host trusts a guest's seeded values. A hash or signature over the save is named future hardening; the seed payload is shaped to accept one additively.
- **A guest's progression on the host's save.** A guest's values persist to the guest's own device only. If a group wants a shared campaign where the host holds everyone's progression, that is a different model needing durable player identity — not scoped here.
- **Per-weapon proficiency.** Expressible only as one declared slot per weapon class. The per-key mechanism `combat-events.md` sketches remains unbuilt and demand-gated.
