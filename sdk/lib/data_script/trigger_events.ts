// Trigger-event and trigger-pool declaration builders. A level binds a trigger
// member's events with `t.on` (`../entities/triggers`); a mod declares a
// standing tag-keyed rule with `defineTriggerEvent`.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

import type { EntityId } from "postretro";
import type { Reaction, TriggerEventParams } from "../data_script";

/** Mod-global trigger event: a standing rule keyed by tag, accepted only in `ModManifest.triggerEvents`. */
export type TriggerEventDescriptor = {
  tag: string;
  event: "enter" | "exit";
  fire: string[];
  levels?: string[];
};

/** Level trigger event: one volume's edge, keyed by the member's id. Built by `t.on`; accepted only in `setupLevel`'s `triggerEvents`. */
export type VolumeTriggerEventDescriptor = {
  trigger: EntityId;
  event: "enter" | "exit";
  fire: string[];
};

export type TriggerPoolDescriptor = {
  tag: string;
  arm?: number;
  armPercentage?: number;
  levels?: string[];
};

/** A reaction a trigger edge fires: a sourceless or trigger-scoped handle, or a bare name. */
export type TriggerEventReaction = Reaction<{}> | Reaction<TriggerEventParams> | string;

/** Authored form of `defineTriggerEvent`: `fire` takes reaction handles or names. */
export type TriggerEventRule = {
  tag: string;
  event: "enter" | "exit";
  fire: TriggerEventReaction[];
  levels?: string[];
};

/** Lower reaction handles to their dispatch names; bare names pass through. */
export function triggerEventFireNames(fire: readonly (string | { readonly name: string })[]): string[] {
  return fire.map((reaction) => typeof reaction === "string" ? reaction : reaction.name);
}

/**
 * Declare a mod-global trigger event: every volume carrying `tag` fires `fire`
 * on `event`, in each level the `levels` selector matches (every level when
 * omitted). Return it from `ModManifest.triggerEvents`; a level script binds
 * its own volumes with a trigger member's `on`.
 */
export function defineTriggerEvent(rule: TriggerEventRule): TriggerEventDescriptor {
  const descriptor: TriggerEventDescriptor = {
    tag: rule.tag,
    event: rule.event,
    fire: triggerEventFireNames(rule.fire),
  };
  if (rule.levels !== undefined) descriptor.levels = rule.levels;
  return descriptor;
}

/** Identity builder for a trigger-pool declaration returned from a level or mod manifest. Engine parsing owns arming validation. */
export function defineTriggerPool(pool: TriggerPoolDescriptor): TriggerPoolDescriptor {
  return pool;
}
