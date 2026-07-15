import {
  type CrossingParams,
  type Reaction,
  type TickParams,
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

void invalidTickRead;
void invalidCrossingRead;
void invalidScopeErasure;
void invalidScopedHelperErasure;
void legalAccumulator;
void invalidReadonlyAccumulator;
void invalidBooleanAccumulator;
