// Reaction and sequence builders: `defineReaction`, the shared author-time
// dispatch params and their opaque target tokens, `wait`/`fire` control steps,
// and map-tag scoping.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

import type {
  CrossingParams,
  EmitterParams,
  EmitterTarget,
  PlayerEventParams,
  Reaction,
  ReactionBody,
  SequenceStep,
  TriggerEventParams,
} from "../data_script";
import { ACTIVATORS_TARGET, PLAYER_TARGET, TRIGGER_TARGET } from "./commands";

type ReactionTracer<S> = (params: S) => ReactionBody;

// This is deliberately one plain merged object rather than a Proxy. The
// subject tokens (activators, trigger, player) sit beside these input nodes and carry
// their own verbs (`./commands`).
// The emitter token carries its own wire spelling, so `playSound` (in the UI
// reaction module) lowers it without importing this module's private tokens.
const EMITTER_TARGET = Object.freeze({ __wire: "@emitter" }) as unknown as EmitterTarget;

export const DISPATCH_PARAMS = Object.freeze({
  rising: Object.freeze({ op: "input", name: "@rising" } as const),
  dt: Object.freeze({ op: "input", name: "@dt" } as const),
  activators: ACTIVATORS_TARGET,
  trigger: TRIGGER_TARGET,
  occupancy: Object.freeze({ op: "input", name: "@occupancy" } as const),
  emitter: EMITTER_TARGET,
  player: PLAYER_TARGET,
});

/**
 * Enroll the rest of this sequence body with the host scheduler and stop; the
 * remaining steps resume after `durationMs` (rounded up to whole authoritative
 * ticks). `interruptible` (default `false`) lets the reaction's paired trigger
 * Exit edge cancel the remaining steps while parked.
 */
export function wait(durationMs: number, opts?: { interruptible?: boolean }): SequenceStep[] {
  return [{
    id: "@wait",
    primitive: "wait",
    args: { durationMs, interruptible: opts?.interruptible ?? false },
  } as SequenceStep];
}

/**
 * Dispatch a named reaction by handle or name from inside a sequence body.
 * `reaction` accepts a `Reaction<{}>` handle or a bare name string, resolved
 * exactly as a trigger member's `on` resolves its `fire` entries. Typing the
 * parameter `Reaction<{}>` rather than `Reaction<S>` makes firing a scoped
 * reaction a compile-time error: a `fire` step dispatches on the app drain
 * with no fire-time dispatch context.
 */
export function fire(reaction: Reaction<{}> | string): SequenceStep[] {
  const event = typeof reaction === "string" ? reaction : reaction.name;
  return [{ id: "@fire", primitive: "fire", args: { event } } as SequenceStep];
}

/**
 * Deterministic, run-stable id derived from a reaction body. Content-derived
 * (a stable string serialization of the body hashed with FNV-1a) so re-running
 * registration yields the same id — crossings and the `onPress` wire form
 * reference it, so it must not vary across runs.
 *
 * NOTE: the auto-id is run-stable within a runtime but NOT identical across
 * TS and Luau — each uses a different stable-stringify implementation. Do not
 * assume cross-runtime id parity; use an explicit `name` when the id must
 * match across both runtimes.
 */
function autoReactionId(descriptor: ReactionBody): string {
  const serialized = stableStringify(descriptor);
  // FNV-1a (32-bit). Deterministic and dependency-free; collision risk is
  // acceptable for author-named reaction ids and an explicit `name` overrides it.
  let hash = 0x811c9dc5;
  for (let i = 0; i < serialized.length; i++) {
    hash ^= serialized.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return `reaction_${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

/** Order-stable JSON serialization: object keys are emitted sorted so two
 * structurally identical bodies always serialize identically. */
function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  const keys = Object.keys(value as Record<string, unknown>).sort();
  const entries = keys.map(
    (k) => `${JSON.stringify(k)}:${stableStringify((value as Record<string, unknown>)[k])}`,
  );
  return `{${entries.join(",")}}`;
}

/**
 * Build a named reaction descriptor. Pure: returns a plain object and performs
 * no FFI. `descriptor` accepts exactly one body shape: `progress`, `primitive`,
 * or `sequence`. `name` is optional; when omitted a deterministic, run-stable id
 * is derived from the body. Use explicit names when TS and Luau scripts must
 * agree. The returned handle can be passed to `Button.onPress` or crossing
 * `fire` entries.
 *
 * @param name Stable event/reaction name consumed by dispatch. Optional.
 * @param descriptor Reaction body data consumed later by Rust.
 */
export function defineReaction(body: ReactionBody): Reaction<{}>;
export function defineReaction(tracer: ReactionTracer<CrossingParams>): Reaction<CrossingParams>;
export function defineReaction(tracer: ReactionTracer<TriggerEventParams>): Reaction<TriggerEventParams>;
export function defineReaction(tracer: ReactionTracer<EmitterParams>): Reaction<EmitterParams>;
export function defineReaction(tracer: ReactionTracer<PlayerEventParams>): Reaction<PlayerEventParams>;
export function defineReaction(
  name: string,
  descriptor: ReactionBody,
): Reaction<{}>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<CrossingParams>,
): Reaction<CrossingParams>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<TriggerEventParams>,
): Reaction<TriggerEventParams>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<EmitterParams>,
): Reaction<EmitterParams>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<PlayerEventParams>,
): Reaction<PlayerEventParams>;
export function defineReaction(
  nameOrBody:
    | string
    | ReactionBody
    | ReactionTracer<CrossingParams | TriggerEventParams | EmitterParams | PlayerEventParams>,
  descriptor?:
    | ReactionBody
    | ReactionTracer<CrossingParams | TriggerEventParams | EmitterParams | PlayerEventParams>,
):
  | Reaction<{}>
  | Reaction<CrossingParams>
  | Reaction<TriggerEventParams>
  | Reaction<EmitterParams>
  | Reaction<PlayerEventParams> {
  const authored = typeof nameOrBody === "string" ? descriptor : nameOrBody;
  const tracedBody = typeof authored === "function"
    ? authored(DISPATCH_PARAMS)
    : authored as ReactionBody;
  const [name, body] =
    typeof nameOrBody === "string"
      ? [nameOrBody, tracedBody]
      : [autoReactionId(tracedBody), tracedBody];
  return { name, ...body } as Reaction<{}> | Reaction<CrossingParams> | Reaction<TriggerEventParams>;
}

/** Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are matched against `ModMapEntry.tags`; omit scoping for every level. */
export function scopeReactions<S>(
  tags: string[],
  list: Reaction<S>[],
): Reaction<S>[] {
  return list.map((reaction) => ({ ...reaction, levels: tags }));
}
