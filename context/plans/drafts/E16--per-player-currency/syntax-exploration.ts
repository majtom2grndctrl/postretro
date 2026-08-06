// E16--per-player-currency — syntax exploration scratchpad
// NOT part of the spec yet. Iterating here before folding anything into
// context/plans/drafts/E16--per-player-currency/index.md.
//
// Status: exploring a Pinia-flavored `.for(owner)` scoping accessor as an
// alternative to both `slot(ref).of(token).add(delta)` and the
// `SourceHandle.slot()/.addSlot()` two-method proposal.
//
// Open questions (unresolved — do not treat anything below as settled):
// - Exact name for `.for()`. Candidates: for, of, scoped, at. "for" reads
//   well at the call site (`progression.for(impact.source)`) but collides
//   informally with JS's `for` keyword in conversation, not in code.
// - Does `.for(owner)` return a NEW object per call (cheap? does it need to
//   be, given the VM drops after setup and this all still lowers to the
//   same per-fire IR — no live allocation cost at *runtime*, only at
//   author-time tracing)?
// - Read confirmed to need the NumberRef/BoolRef fluent family (GatedEffect.
//   when: BoolRef, data_script.ts:216) — plain RuntimeValue (runtime.*) does
//   NOT interoperate, numberRef() wrapper is not exported. So `.for()`'s
//   returned per-slot accessors must themselves be real NumberRef-producing,
//   same as SourceHandle.slot() would have been.
// - `id`/name strings (defineImpactEvent, defineStore's namespace) stay
//   required — the VM drops after setup, nothing survives except serialized
//   data, and `.override()` mints a second object reusing the same `id`
//   string (data_script.ts:397-408), which a JS variable binding can't do.

// ---------------------------------------------------------------------------
// Variant A — `.for(owner)` scoping accessor, addressed once
// ---------------------------------------------------------------------------

const progression = defineStore("player", {
  xp: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  teamKills: { type: "number", default: 0 },
});

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const mine = progression.for(impact.source); // owner-scoped instance, ~Pinia's useStore()

  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const base = impact.target.healthAfter.le(-40).select(50, 25);
  const bonus = mine.xp.gt(100).select(base.times(2), base); // read — already owner-bound

  return [
    { when: killed, do: [
        mine.xp.add(bonus),              // write — per player, addressed once by `.for(...)`
        progression.teamKills.add(1),    // shared — untouched, no scoping needed
    ]},
  ];
});

// ---------------------------------------------------------------------------
// Variant A2 — named actions (Pinia-flavored, bigger departure — new
// defineStore schema surface, likely out of spec scope, kept here for the
// idea only)
// ---------------------------------------------------------------------------

// const progression2 = defineStore("player", {
//   state: { xp: { type: "number", default: 0, perOwner: true } },
//   actions: {
//     awardXp(amount) { this.xp.add(amount); },
//   },
// });
// mine.awardXp(bonus)  // instead of mine.xp.add(bonus) — names the domain verb, not the mechanism

// ---------------------------------------------------------------------------
// Reaction-side write — unaffected by any of the above. Reactions have no
// `impact.source`; stays a flat top-level call addressed by activators/tag.
// ---------------------------------------------------------------------------

onTriggerEvent({ tag: "objective" }, "enter", [
  defineReaction((on: TriggerEventParams) => addSlot(on.activators, progression.xp, 100)),
]);
