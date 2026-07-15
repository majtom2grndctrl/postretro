import {
  type CrossingParams,
  type Reaction,
  type TickParams,
  runtime,
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

void invalidTickRead;
void invalidCrossingRead;
void invalidScopeErasure;
