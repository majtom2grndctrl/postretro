// Player-event builders: the `becomes` / `ceases` edge words and the
// `players().on` descriptor. Pure data; effective only when returned under a
// manifest's `playerEvents`.
// See: context/lib/scripting.md §12 (Player events)

import type { RuntimeValue } from "postretro";
import type { BoolRef, PlayerEventParams, Reaction } from "../data_script";
import { boolNode } from "../util/expression_refs";
import { triggerEventFireNames } from "./trigger_events";

/** The edge a player event fires on: `becomes` (false → true) or `ceases` (true → false). */
export type PlayerEventEdgeWord = "becomes" | "ceases";

declare const playerEventEdgeBrand: unique symbol;

/** A condition paired with its edge word. Build it with `becomes(cond)` or `ceases(cond)`. */
export type PlayerEventEdge = Readonly<{
  readonly [playerEventEdgeBrand]: true;
}>;

/** A reaction a player event fires: a sourceless or player-event-scoped handle, or a bare name. */
export type PlayerEventReaction = Reaction<{}> | Reaction<PlayerEventParams> | string;

/** `levels` scopes a `ModManifest` entry to matching map tags; a level script's entry must omit it. */
export type PlayerEventOptions = { levels?: string[] };

/** One `playerEvents` entry, as `players().on` builds it. */
export type PlayerEventDescriptor = {
  edge: PlayerEventEdgeWord;
  condition: RuntimeValue;
  fire: string[];
  levels?: string[];
};

type EdgeData = { edge: PlayerEventEdgeWord; condition: RuntimeValue };

// Only edges built here lower; a hand-built `{ edge, condition }` is rejected
// at author time rather than reaching the engine as a guess.
const edges = new WeakMap<object, EdgeData>();

function edgeWord(edge: PlayerEventEdgeWord, cond: BoolRef): PlayerEventEdge {
  const handle = Object.freeze({}) as PlayerEventEdge;
  edges.set(handle, { edge, condition: boolNode(cond) });
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

/** Build the `players().on` descriptor. */
export function playerEvent(
  edge: PlayerEventEdge,
  fire: PlayerEventReaction[],
  options?: PlayerEventOptions,
): PlayerEventDescriptor {
  const data = edges.get(edge as object);
  if (data === undefined) {
    throw new TypeError("players().on: `edge` must come from becomes(cond) or ceases(cond)");
  }
  const descriptor: PlayerEventDescriptor = {
    edge: data.edge,
    condition: data.condition,
    fire: triggerEventFireNames(fire),
  };
  if (options?.levels !== undefined) descriptor.levels = options.levels;
  return descriptor;
}
