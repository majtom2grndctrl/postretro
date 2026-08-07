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
// Correction (verified against source): the reaction-side write below was
// invented, not real. `addSlot` does not exist. What DOES exist is a
// free-function precedent for reactions:
//
//   export function grantHealth(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor
//     (data_script.ts:514-523)
//
// `ActivatorsTarget` (data_script.ts:18-22) is `Readonly<{ [brand]: true }>` —
// zero exposed data fields, checked by identity against a private singleton
// (`target === ACTIVATORS_TARGET`, :509/521/534). The wire string "@activators"
// is baked into the function body, not read off the handle. `SourceHandle`
// (data_script.ts:233-247, singleton built :357-360) is the same shape:
// `{ [sourceBrand]: true }`, no token field, methods closed over the wire
// literal "@impact.source" internally.
//
// So PostRetro already has a *shape* precedent — top-level functions that take
// a handle as an argument, not methods living on the handle (reactions'
// grantHealth(target, amount) vs. impact policy's SourceHandle.grantHealth
// (amount)) — but neither handle today is a real token-carrying value the way
// WritableStateRef (`{ slot: string }`, data_script.ts:774) already is.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Variant B — SourceHandle restructured as a token-carrying value, consumed
// by free functions. Makes "handle is the ID" hold the same way on the
// owner/source side as it already does on the slot side (WritableStateRef).
// ---------------------------------------------------------------------------

// Proposed shape (mirrors WritableStateRef's `{ slot: string }` exactly):
//
//   export type SourceHandle = Readonly<{
//     readonly token: "@impact.source";
//     readonly [sourceBrand]: true;
//   }>;
//
// grantHealth/grantAmmo move off the handle and become free functions,
// matching the reaction side's existing shape:
//
//   export function grantHealth(source: SourceHandle, amount: NumberValue): Effect { ... }
//   export function grantAmmo(source: SourceHandle, type: string, amount: NumberValue): Effect { ... }
//
// For this spec, the same free-function shape covers the new per-owner slot
// write, and the `.for(owner)` scoping accessor from Variant A becomes
// unnecessary — the owner token is just another argument, not a wrapper object:
//
//   addSlot(impact.source, progression.xp, bonus)   // source: SourceHandle, ref: WritableStateRef<number>

const reward2 = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const base = impact.target.healthAfter.le(-40).select(50, 25);

  return [
    { when: killed, do: [
        addSlot(impact.source, progression.xp, base),   // per-owner write, source handle carries its own token
        addSlot(progression.teamKills, base),            // shared write, no owner argument — needs a second overload or a distinct free fn
    ]},
  ];
});

// Open problem with Variant B: the shared (non-owner) write above doesn't
// have a handle to pass — `progression.teamKills` alone identifies the slot,
// not who's writing it. Either `addSlot` needs two signatures (owner-scoped
// vs. global), or global writes keep using `slot(ref).add(delta)` and only
// the owner-scoped path gets the new free function. Unresolved — pick before
// this goes in the spec.

// ---------------------------------------------------------------------------
// Reaction-side write — unaffected by any of the above. Reactions have no
// `impact.source`; stays a flat top-level call addressed by activators/tag,
// using the real, verified `grantHealth(target, amount)` free-function shape.
// ---------------------------------------------------------------------------

onTriggerEvent({ tag: "objective" }, "enter", [
  defineReaction((on: TriggerEventParams) => grantHealth(on.activators, 25)),
]);
