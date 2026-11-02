// Player-event builders: the `becomes` / `ceases` edge words, the discrete
// `assistReceived` / `assistGiven` sources, and the `players().on` descriptor. Pure data; effective only when returned under a
// manifest's `playerEvents`.
// See: context/lib/scripting.md §12 (Player events)

import type { RuntimeValue } from "postretro";
import type { BoolRef, PlayerEventParams, Reaction } from "../data_script";
import { boolNode, loweredValue } from "../util/expression_refs";
import { triggerEventFireNames } from "./trigger_events";

/** The edge a player event fires on: `becomes` (false → true) or `ceases` (true → false). */
export type PlayerEventEdgeWord = "becomes" | "ceases";

/** A discrete player-event source: an assist completed on this player (`assistReceived`) or by this player (`assistGiven`). */
export type PlayerEventDiscreteWord = "assistReceived" | "assistGiven";

declare const playerEventEdgeBrand: unique symbol;

/** What a player event fires on. Build it with `becomes(cond)`, `ceases(cond)`, `assistReceived()` or `assistGiven()`. */
export type PlayerEventEdge = Readonly<{
  readonly [playerEventEdgeBrand]: true;
}>;

/** A reaction a player event fires: a sourceless or player-event-scoped handle, or a bare name. */
export type PlayerEventReaction = Reaction<{}> | Reaction<PlayerEventParams> | string;

/** `levels` scopes a `ModManifest` entry to matching map tags; a level script's entry must omit it. */
export type PlayerEventOptions = { levels?: string[] };

/** One `playerEvents` entry, as `players().on` builds it. A discrete source carries no condition. */
export type PlayerEventDescriptor =
  | { edge: PlayerEventEdgeWord; condition: RuntimeValue; fire: string[]; levels?: string[] }
  | { edge: PlayerEventDiscreteWord; fire: string[]; levels?: string[] };

type EdgeData =
  | { edge: PlayerEventEdgeWord; condition: RuntimeValue }
  | { edge: PlayerEventDiscreteWord };

// Only edges built here lower; a hand-built `{ edge, condition }` is rejected
// at author time rather than reaching the engine as a guess.
const edges = new WeakMap<object, EdgeData>();

// Any fluent value (even a NumberRef that slipped past the types) lowers to IR,
// so the engine's "condition must produce Bool" check names the event instead
// of a malformed-descriptor error.
function lowerCondition(cond: BoolRef): RuntimeValue {
  return typeof cond === "boolean" ? boolNode(cond) : (loweredValue(cond) as RuntimeValue);
}

function edgeWord(edge: PlayerEventEdgeWord, cond: BoolRef): PlayerEventEdge {
  const handle = Object.freeze({}) as PlayerEventEdge;
  edges.set(handle, { edge, condition: lowerCondition(cond) });
  return handle;
}

/** Fire when `cond` turns true for a player. A player first observed while `cond` holds fires too. */
export function becomes(cond: BoolRef): PlayerEventEdge {
  return edgeWord("becomes", cond);
}

/** Fire when `cond` turns false for a player who was observed with it true. */
export function ceases(cond: BoolRef): PlayerEventEdge {
  return edgeWord("ceases", cond);
}

function discrete(edge: PlayerEventDiscreteWord): PlayerEventEdge {
  const handle = Object.freeze({}) as PlayerEventEdge;
  edges.set(handle, { edge });
  return handle;
}

/** Fire on the target when an assist completes on them; `on.player` is the target. Host only; carries no condition. */
export function assistReceived(): PlayerEventEdge {
  return discrete("assistReceived");
}

/** Fire on the helper when their assist completes; `on.player` is the helper. Host only; carries no condition. */
export function assistGiven(): PlayerEventEdge {
  return discrete("assistGiven");
}

/** Build the `players().on` descriptor. */
export function playerEvent(
  edge: PlayerEventEdge,
  fire: PlayerEventReaction[],
  options?: PlayerEventOptions,
): PlayerEventDescriptor {
  const data = edges.get(edge as object);
  if (data === undefined) {
    throw new TypeError(
      "players().on: `edge` must come from becomes(cond), ceases(cond), assistReceived() or assistGiven()",
    );
  }
  const descriptor: PlayerEventDescriptor = { ...data, fire: triggerEventFireNames(fire) };
  if (options?.levels !== undefined) descriptor.levels = options.levels;
  return descriptor;
}
