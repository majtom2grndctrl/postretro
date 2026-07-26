# Per-Player Currency (E16)

## Goal

A mod currency — XP, credits, per-weapon proficiency — can only be a global store slot today, so in co-op every player shares one pot. Give authors the choice: keep the shared slot, or hold the currency per player. Two pieces make that possible — the impact policy must be able to write per-entity state on the damage **source** (not just the target it lands on), and a per-entity state field must be able to reach its owner's HUD. Instancing then becomes a storage-location choice, not a new grant parameter.

## Prerequisites

- **`E16--resource-grant-chokepoint`** — pins that a grant addresses the source and widens the effect-target check from a single-token guard to an expected-token checker. This spec's source-addressed state write reuses that decision and that checker; landing them the other way round would decide source-targeting twice.
- **`E16--impact-policy-substrate`** (shipped) — the per-entity-state keystone, the composite binding scope, the per-fire snapshot discipline, and the evaluate-then-apply model.
- **Epic 15 Phase 3.5** (shipped) — the owner-private replication channel and the pawn↔client owner map this projects through.

## Scope

### In scope

- **Source-addressed per-entity state.** Read and write a named per-entity number field on the impact's source, alongside the shipped target-addressed path. This settles the cross-entity write path `scripting.md` §11 flags as unsettled.
- **A per-player slot binding.** A number slot declares that its per-player value comes from a named per-entity state field on the owning pawn; the engine projects it.
- **Both publish paths.** Host / single-player through the HUD slot publisher, connected clients through the owner-private replication projection — the two-sided shape `player.health` already has.
- **Reference per-player XP** in the dev mod, with a HUD readout.

### Out of scope

- **General cross-entity addressing.** Only the two entities the impact dispatch publishes — target and source — are addressable. Marking an arbitrary third entity for another expression to read stays unexpressible.
- **Per-key / per-category slots.** `xpByWeapon.shotgun` as sketched in `combat-events.md` needs a per-key slot mechanism that was never built. A per-weapon bucket is expressible today only as one declared field per weapon class.
- **Client-authored writes.** The projection is host → owner. A client never writes a per-player slot; it reads a replicated view.
- **Persisting per-entity state.** Store slots persist; per-entity state does not, and a projected slot is readonly, so a per-player currency does not survive a save today.
- **Replicating per-entity state generally.** Only fields a slot declaration binds are projected. The keystone stays host-authoritative otherwise, as its spec pinned.
- **Engine-owned resources.** Health and ammo are the grant spec's; this spec touches mod currency only.

## Direction

**Problem.** The impact policy can write per-entity state only on the entity the impact landed on, and a mod currency can live only in a global store slot. A kill credits the *killer*, who is the source — so per-player reward is unwritable at the policy layer and invisible at the HUD layer. Both halves are the same missing capability seen from two ends: nothing addresses the source's own state.

**Prior commitments.**
- *The keystone is deliberately emergent.* `E16--impact-policy-substrate` made per-entity state implicit-on-first-write with a total-zero default, so any scope can graft a field onto an entity its author never anticipated. Source-addressed writes extend that reach without adding a declaration surface — an unset source field still reads zero.
- *Same-entity-by-construction was documented as provisional.* `scripting.md` §11: "Marking entity A for entity B's expression to read is not expressible today, and a scope that needs it must settle the write path first." This is that scope, and this is that settlement. **The divergence is deliberate and narrow:** writer and reader stay inside one impact fire, and the addressable set is exactly the two tokens the dispatch already publishes — not an open entity-addressing facility. `scripting.md` §11 wants updating at promotion to say so.
- *Currencies are mod-owned.* `combat-events.md` §2 — a currency is a store write, freely authored, never a blessed engine resource. Honored: this spec adds no engine currency, only a place to put a mod one.
- *Owner-private slots project from host components.* Phase 3.5 established that an owner-private slot's per-owner value is read off the owning pawn rather than fanned out from one global value. A per-entity state field is one more such source.

**Placement.** The per-player view is a *slot projection*, not a new replication channel and not a new store kind. Slots are already the UI's binding namespace and already carry a scope; per-entity state is already per-instance. Putting the seam at the projection means the HUD, the wire schema, and the persistence rules all stay untouched — the only new thing is where one slot's value is read from. The alternative placements (a new per-player store, a per-entity replication channel) would each duplicate machinery that exists.

**Alternatives rejected.**
- *Owner-private store slots with a per-key suffix* — declare `progression.xp` as owner-private and give it one instance per player. This is the `combat-events.md` sketch. Rejected: store slots are addressed by a single global dotted name, so per-player instances require the `perKey` mechanism that was never built, plus a new per-owner write path. Per-entity state is already per-instance and already writable from a policy; the slot only needs to *view* it.
- *Making XP an engine-owned resource granted through the chokepoint.* Would need no new write path at all. Rejected: it inverts the epic's central split — the engine would own a currency and hold an opinion about reward, which is the thing `combat-events.md` exists to prevent.
- *A per-entity replication channel.* Replicate per-entity state wholesale to owners. Rejected as unbounded: every field on every entity becomes wire traffic, and the substrate scoped exactly this out. A declaration-gated projection replicates only what an author asked for.

## Decisions

- **Two addressable tokens, one snapshot discipline.** A policy reads and writes per-entity state on the target (shipped) or the source (new). Source-addressed reads are served from a per-fire frozen snapshot captured at fire time, exactly as target reads are — so a gate never observes a write made in its own fire, and the evaluate-then-apply model is unchanged.
- **A source-addressed write with no source is a skip; a source-addressed read with no source is total-zero.** Consistent with the two halves already: unset per-entity state reads zero, and an effect with no resolvable recipient skips. The asymmetry is deliberate — a read must produce a value for the IR's totality contract, a write has nothing to write to.
- **Self-damage collapses the two tokens.** When source and target are the same entity, both channels address it and both snapshots hold the same values. Two writes to one field in a single fire apply in effect order, last write winning. Pinned rather than forbidden: forbidding it would mean a bind-time check on a runtime identity.
- **One declaration, not two flags.** A number slot declares its per-player binding by naming the per-entity state field it views. That single declaration implies owner-private scope and readonly-to-scripts; there is no way to declare a per-player slot that is not owner-private, or one whose scope and source disagree. Authority is the per-entity state field — writes go through the impact policy, reads through the slot.
- **A per-player slot is readonly and non-persisting.** Readonly because its authority lives on the pawn; non-persisting because per-entity state does not survive a level change. A declaration that also asks to persist is a load error naming the slot, not a silent drop.
- **Both publish paths or neither.** Host and single-player publish from the local pawn through the HUD slot publisher; connected clients receive the owner-private projection. A per-player slot that worked only in co-op would be a trap, since the dev walkthrough is single-player.
- **No pawn, no write.** A player with no pawn, or a pawn with no state component, leaves the slot at its previous value — the accepted slot-staleness contract the health publisher already follows, not a reset to the default.

## Acceptance criteria

- [ ] An impact policy writes a per-entity state field on the damage source; a later impact from the same source reads the accumulated value and branches on it.
- [ ] Two different sources damaging the same target accrue independent values — the field is per source instance, not per target and not global.
- [ ] A source-addressed read on a fire with no source yields zero and the policy still evaluates; a source-addressed write on such a fire is skipped without disturbing sibling effects.
- [ ] A gate reading source state does not observe a write made by its own fire — the pre-effect snapshot rule holds for the source channel as it does for the target channel.
- [ ] Self-damage (source and target the same entity) applies both channels' writes in effect order, ending at the last-written value.
- [ ] A slot declared with a per-player binding reads the owning pawn's named state field in single-player, and the HUD shows it changing as the policy writes.
- [ ] In co-op, each client's slot reflects only its own pawn's field; a second client's value never leaks across.
- [ ] A per-player slot rejects script writes, and a declaration that also requests persistence fails at load with a diagnostic naming the slot.
- [ ] A player with no pawn leaves the slot at its previous value rather than resetting it.
- [ ] Reference walkthrough: killing enemies raises a visible per-player XP readout, and the same policy re-pointed at a global slot produces shared XP — the storage location is the only edit.

## Tasks

### Task 1: Source-addressed per-entity state

Extend the composite binding scope so a policy can address per-entity state on the impact source as well as the target. The scope today holds one target id plus a bound state-name list and a parallel snapshot refreshed per fire (`crates/scripting-core/src/ir/scopes.rs` — `seed_impact_from_registry` seeds the numeric dispatch facts then calls the target-seeding path, which freezes every bound state field off the target's state component). Add the mirror: a source id and its own bound-name list and snapshot, seeded in the same call from the dispatch's optional source, with an absent source freezing every source field to zero. Pin a reserved wire prefix for the source channel distinct from the shipped target prefix, and route it at bind time so a name can never fall through to the global store — the same prefix-routing rule the target channel established. Reads resolve to input leaves against the frozen snapshot; writes resolve to output handles that apply to the source id at apply time and skip when it is absent. Both channels share one freeze point, so the evaluate-then-apply guarantee covers them together. Cover in tests: independent accumulation across two sources, the absent-source read and write, the snapshot rule for the source channel, and the self-damage collapse.

### Task 2: Authoring surface for source state

Add `state(name)` and `setState(name, value)` to the SDK's source handle — which the grant spec has already populated with the grant arms — mirroring the shipped target-handle pair in shape and documentation, and mirror both in the Luau typedef. Lower them to the reserved source prefix from Task 1. In the impact-policy bind path (`crates/postretro/src/impact_policy.rs`), the `setState` arm currently accepts only the impact-target token and errors otherwise; accept the source token as well and route it to the source output channel, reusing the expected-token checker the grant spec generalizes rather than adding a third bespoke guard. Document on the source-handle methods that an impact with no damager reads zero and skips the write, and that app-drain impacts run no policy in v1 — the same two caveats the grant arms carry, since both hang off the same token.

### Task 3: Per-player slot binding and projection

Give a number slot a per-player binding: a declaration naming the per-entity state field it views on the owning pawn, which implies owner-private scope and readonly, and which rejects a co-declared persist request at load with a diagnostic naming the slot. Thread it through the slot schema, the SDK store-slot type and its generated typedef, and the descriptor parsers in both runtimes. Then wire both publish paths, mirroring how health already reaches each. **Host / single-player:** the HUD slot publisher (`crates/postretro/src/scripting/systems/ui_proxy.rs`) republishes player slots each frame from the local pawn — add the per-player-bound slots there, reading the named field off the pawn's state component and following the same no-pawn skip that leaves the slot at its previous value rather than resetting it. **Connected clients:** the owner-private source resolver (`crates/postretro/src/netcode/state_slots.rs` — `owner_private_source_value`, which already dispatches by name through the health, weapon-cooldown, and ammo projections before falling through to the global slot table) gains a per-entity-state projection reading the named field off that owner's pawn; it must come before the fall-through so a per-player slot never serves the global value to every owner. That file's non-test body is past the size guidance — add the projection as a sibling helper alongside the existing ones and do not restructure. Test the isolation case explicitly: two clients, two pawns, two values, no leak.

### Task 4: Reference per-player XP

Extend the dev mod's reward policy — the one the grant spec adds to `content/dev/scripts/combat-lifecycle.ts` — to also accrue XP on the killer via a source-addressed `setState`, and declare the per-player slot that views it in the dev mod's store. Add the readout to the dev HUD (`content/dev/scripts/hud.ts`). Comment the policy to show the instancing choice explicitly: the same reward written to a global store slot is shared XP, written to source state it is per-player, and the edit is the destination alone. Update the combat demo README walkthrough.

## Sequencing

**Phase 1 (sequential):** Task 1 — the scope channel everything else binds against.
**Phase 2 (concurrent):** Task 2, Task 3 — the authoring surface and the projection are independent over Task 1 and touch disjoint files.
**Phase 3 (sequential):** Task 4 — consumes both.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A gate never observes a write from its own fire | Task 1 (both channels freeze at one point) | A source snapshot refreshed at apply time instead of fire time would break it silently | AC 4 |
| Per-entity state stays per instance | Task 1 (source id resolved per fire) | Caching a bound source id across fires would alias two sources | AC 2 |
| A per-player slot's authority is the pawn's field, never a script write | Task 3 (readonly implied by the binding) | Any writable path to the slot desyncs it from the field that feeds it | AC 8 |
| A per-player slot never serves one global value to every owner | Task 3 (projection ordered before the global fall-through) | The resolver's fall-through is the threat — ordering is the guard | AC 7 |
| Only declaration-bound fields replicate | Task 3 | A future general per-entity replication channel would void the bound | AC 7 |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| source state read | source channel input leaf | reserved source-state prefix (distinct from the target prefix) | `impact.source.state(name)` | `impact.source:state(name)` |
| source state write | source channel output | effect `primitive: "setState"` with the source token | `impact.source.setState(name, value)` | `impact.source:setState(name, value)` |
| per-player slot binding | slot schema field | slot declaration key naming the state field | store slot declaration field | same |
| replication scope | existing owner-private scope | existing owner-private scope tag | implied by the binding — not separately declared | same |

## Script syntax examples

```ts
// Proposed design — instancing is the destination, not a grant parameter.
const { state: progression } = defineStore("progression", {
  // Shared: one pot for the whole team.
  teamScore: { type: "number", default: 0, network: "shared" },
  // Per player: a view of the `xp` field on the owning pawn. Owner-private and
  // readonly by construction; the policy below is what writes it.
  xp: { type: "number", default: 0, perPlayer: "xp" },
});

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const bonus = impact.target.healthAfter.le(-40).select(50, 25);
  return [
    { when: killed, do: [
        // Per-player: accrues on the killer's own pawn.
        impact.source.setState("xp", impact.source.state("xp").plus(bonus)),
        // Shared: the same reward, one line different.
        progression.teamScore.add(1),
    ]},
  ];
});
```

## Open questions

- **Persistence of a per-player currency.** A projected slot is readonly and non-persisting, so per-player XP does not survive a save while a shared global slot does. Acceptable for a reference policy; a campaign that wants persistent per-player progression needs a save story for per-entity state, which is unscoped work.
- **`scripting.md` §11 update at promotion.** The same-entity-by-construction paragraph becomes wrong once Task 1 lands. It should say the impact scope addresses exactly the two tokens its dispatch publishes, and that general entity addressing remains unexpressible.
- **Per-weapon proficiency.** `combat-events.md`'s `xpByWeapon.shotgun` needs per-key slots. Expressible here only as one declared field per weapon class, which does not scale past a handful. Left to the per-key slot mechanism if a mod demands it.
