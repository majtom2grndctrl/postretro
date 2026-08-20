import {
  type CrossingParams,
  type Reaction,
  type TickParams,
  type TriggerEventParams,
  armTrigger,
  damage,
  defineReaction,
  fire,
  onTriggerEvent,
  runtime,
  scopeReactions,
} from "postretro";
import { onStateCrossing } from "postretro/ui";

declare const unscoped: Reaction<{}>;
declare const crossingScoped: Reaction<CrossingParams>;

// Unscoped reactions are valid at every source, including scoped crossings.
onStateCrossing(runtime.constant(true), [unscoped]);

// @ts-expect-error TickParams intentionally does not publish crossing direction.
const invalidTickRead = (t: TickParams) => t.rising;

// @ts-expect-error CrossingParams intentionally does not publish tick delta.
const invalidCrossingRead = (on: CrossingParams) => on.dt;

// @ts-expect-error Scoped reactions cannot be erased to an unscoped reaction.
const invalidScopeErasure: Reaction<{}> = crossingScoped;

// @ts-expect-error `fire()` takes `Reaction<{}>`, not `Reaction<S>`: no resumed
// step reads fire-time context, so firing a scoped reaction from a sequence
// is a compile error (O30, scripting.md §12).
const invalidFireOfScopedReaction = fire(crossingScoped);

const scopedCrossingList = scopeReactions(["arena"], [crossingScoped]);
// @ts-expect-error Adding level tags must not erase a reaction's dispatch scope.
const invalidScopedHelperErasure: Reaction<{}> = scopedCrossingList[0];

// Accumulators are valid only on writable Number slots.
const legalAccumulator: import("postretro").StoreSlotSchema = {
  type: "number",
  default: 0,
  accumulate: (t: TickParams) => t.dt,
};
// @ts-expect-error readonly Number slots cannot accumulate.
const invalidReadonlyAccumulator: import("postretro").StoreSlotSchema = { type: "number", readonly: true, default: 0, accumulate: (t: TickParams) => t.dt };
// @ts-expect-error non-Number slots cannot accumulate.
const invalidBooleanAccumulator: import("postretro").StoreSlotSchema = { type: "boolean", default: false, accumulate: (t: TickParams) => t.dt };
// @ts-expect-error shared replication serializes one global scalar, so it cannot carry per-owner cardinality.
const invalidSharedPerOwner: import("postretro").StoreSlotSchema = { type: "number", default: 0, perOwner: true, network: "shared" };

const triggerScoped = defineReaction((on: TriggerEventParams) => damage(on.activators, 25));
const triggerSequence = defineReaction((on: TriggerEventParams) => ({ sequence: armTrigger(on.trigger) }));
onTriggerEvent({ tag: "plate" }, "enter", [unscoped, triggerScoped, triggerSequence]);

// @ts-expect-error Trigger-scoped reactions cannot be fired by a state crossing.
onStateCrossing(runtime.constant(true), [triggerScoped]);

defineReaction((on: TriggerEventParams) => {
  // @ts-expect-error The fired trigger token is not a damage target.
  const wrongTarget = damage(on.trigger, 5);
  // @ts-expect-error Opaque entity targets are not runtime IR operands.
  const wrongOperand = runtime.add(on.activators, 1);
  return wrongTarget;
});

void invalidTickRead;
void invalidCrossingRead;
void invalidScopeErasure;
void invalidFireOfScopedReaction;
void invalidScopedHelperErasure;
void legalAccumulator;
void invalidReadonlyAccumulator;
void invalidBooleanAccumulator;
void invalidSharedPerOwner;
void triggerScoped;
void triggerSequence;
