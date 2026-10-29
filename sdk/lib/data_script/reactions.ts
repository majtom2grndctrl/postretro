// Reaction and sequence builders: `defineReaction`, the shared author-time
// dispatch params and their opaque target tokens, `wait`/`fire` control steps,
// and map-tag scoping.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

import type {
  CrossingParams,
  EmitterParams,
  EmitterTarget,
  ManifestReactionBody,
  PlayerEventParams,
  Reaction,
  ReactionBody,
  ReactionStepEntry,
  SequenceStep,
  TriggerEventParams,
  TeleportEventParams,
} from "../data_script";
import { ACTIVATORS_TARGET, PLAYER_TARGET, TRIGGER_TARGET, TELEPORTER_TARGET } from "./commands";

type ReactionTracer<S> = (params: S) => ReactionBody<S>;

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
  teleporter: TELEPORTER_TARGET,
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
function autoReactionId(descriptor: ManifestReactionBody<any>): string {
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

// Thrown for a body that is none of `progress`, `primitive`, `sequence` or an
// array of steps, so a misspelled key never installs an empty reaction. Every
// `defineReaction` message here matches the Luau SDK's word for word.
const BODY_SHAPE_ERROR =
  "defineReaction: a body must be `progress`, `primitive`, `sequence` or a list of steps";

// A body key on an array beside its steps: the author meant one or the other.
function rejectMixed(list: readonly unknown[]): void {
  const keyed = list as unknown as Record<string, unknown>;
  for (const key of ["primitive", "progress", "sequence"]) {
    if (keyed[key] !== undefined) {
      throw new Error(`defineReaction: a list of steps cannot also carry \`${key}\``);
    }
  }
}

/** Append `list`'s steps to `out` in authored order, flattening nested arrays
 * at any depth. An object holding `primitive` is a step and an array is
 * recursed into; anything else throws, naming `defineReaction`. */
function flattenSteps(list: readonly unknown[], out: ReactionStepEntry[]): ReactionStepEntry[] {
  for (const entry of list) {
    if (entry === null || typeof entry !== "object") {
      throw new Error(`defineReaction: a step must be an object, got ${entry === null ? "null" : typeof entry}`);
    }
    const step = entry as Record<string, unknown>;
    if (step.name !== undefined) {
      throw new Error(
        `defineReaction: reaction \`${String(step.name)}\` is a reaction handle, not a step; fire it with \`fire(r)\``,
      );
    }
    if (step.progress !== undefined) {
      throw new Error("defineReaction: a `progress` body is not a step");
    }
    if (step.sequence !== undefined) {
      throw new Error("defineReaction: a nested `{ sequence }` is not a step; list its steps instead");
    }
    if (Array.isArray(entry)) {
      rejectMixed(entry);
      flattenSteps(entry, out);
      continue;
    }
    if (step.primitive === undefined) {
      throw new Error("defineReaction: an entry with no `primitive` is not a step");
    }
    if (step.onComplete !== undefined) {
      throw new Error(`defineReaction: step \`${String(step.primitive)}\` carries \`onComplete\`, which only a body may`);
    }
    out.push(entry as ReactionStepEntry);
  }
  return out;
}

/** Lower an authored body to the manifest's shape: an array, plain or
 * returned from the tracer, is a `{ sequence }`, and every `sequence` list is
 * flattened. */
function lowerBody(body: ReactionBody<any>): ManifestReactionBody<any> {
  if (Array.isArray(body)) {
    rejectMixed(body);
    return { sequence: flattenSteps(body, []) };
  }
  if (body === null || typeof body !== "object") {
    throw new Error(BODY_SHAPE_ERROR);
  }
  const authored = body as Record<string, unknown>;
  if (authored.sequence !== undefined) {
    if (!Array.isArray(authored.sequence)) {
      throw new Error("defineReaction: `sequence` must be a list of steps");
    }
    rejectMixed(authored.sequence);
    return { ...authored, sequence: flattenSteps(authored.sequence, []) } as ManifestReactionBody<any>;
  }
  if (authored.progress === undefined && authored.primitive === undefined) {
    throw new Error(BODY_SHAPE_ERROR);
  }
  return body as ManifestReactionBody<any>;
}

/**
 * Build a named reaction descriptor. Pure: returns a plain object and performs
 * no FFI. `descriptor` accepts one body shape: `progress`, `primitive`,
 * `sequence`, or an array of steps, which lowers to a `sequence`; nested
 * arrays flatten. Any other shape throws. `name` is optional; when omitted a
 * deterministic, run-stable id is derived from the lowered body. Use explicit
 * names when TS and Luau scripts must agree. The returned handle can be passed
 * to `Button.onPress` or crossing `fire` entries.
 *
 * @param name Stable event/reaction name consumed by dispatch. Optional.
 * @param descriptor Reaction body data consumed later by Rust.
 */
export function defineReaction<S = {}>(body: ReactionBody<S>): Reaction<S>;
export function defineReaction(tracer: ReactionTracer<CrossingParams>): Reaction<CrossingParams>;
export function defineReaction(tracer: ReactionTracer<TriggerEventParams>): Reaction<TriggerEventParams>;
export function defineReaction(tracer: ReactionTracer<TeleportEventParams>): Reaction<TeleportEventParams>;
export function defineReaction(tracer: ReactionTracer<EmitterParams>): Reaction<EmitterParams>;
export function defineReaction(tracer: ReactionTracer<PlayerEventParams>): Reaction<PlayerEventParams>;
export function defineReaction<S = {}>(
  name: string,
  descriptor: ReactionBody<S>,
): Reaction<S>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<CrossingParams>,
): Reaction<CrossingParams>;
export function defineReaction(
  name: string,
  tracer: ReactionTracer<TriggerEventParams>,
): Reaction<TriggerEventParams>;
export function defineReaction(name: string, tracer: ReactionTracer<TeleportEventParams>): Reaction<TeleportEventParams>;
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
    | ReactionBody<any>
    | ReactionTracer<any>,
  descriptor?:
    | ReactionBody<any>
    | ReactionTracer<any>,
): Reaction<any> {
  const authored = typeof nameOrBody === "string" ? descriptor : nameOrBody;
  const tracedBody = lowerBody(typeof authored === "function"
    ? authored(DISPATCH_PARAMS)
    : authored as ReactionBody<any>);
  const [name, body] =
    typeof nameOrBody === "string"
      ? [nameOrBody, tracedBody]
      : [autoReactionId(tracedBody), tracedBody];
  return { name, ...body } as Reaction<any>;
}

/** Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are matched against `ModMapEntry.tags`; omit scoping for every level. */
export function scopeReactions<S>(
  tags: string[],
  list: Reaction<S>[],
): Reaction<S>[] {
  return list.map((reaction) => ({ ...reaction, levels: tags }));
}
