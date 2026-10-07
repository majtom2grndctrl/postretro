// Trigger-event and trigger-pool declaration builders.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

import type { Reaction, TriggerEventParams } from "../data_script";

export type TriggerEventDescriptor = {
  tag: string;
  event: "enter" | "exit";
  fire: string[];
  levels?: string[];
};

export type TriggerPoolDescriptor = {
  tag: string;
  arm?: number;
  armPercentage?: number;
  levels?: string[];
};

export type TriggerEventOptions = { levels?: string[] };

type TriggerEventReaction = Reaction<{}> | Reaction<TriggerEventParams> | string;

/** Build a trigger-event observer descriptor, lowering reaction handles to names. */
export function onTriggerEvent(
  filter: { tag: string },
  event: "enter" | "exit",
  fire: TriggerEventReaction[],
  options?: TriggerEventOptions,
): TriggerEventDescriptor {
  const descriptor: TriggerEventDescriptor = {
    tag: filter.tag,
    event,
    fire: fire.map((reaction) => typeof reaction === "string" ? reaction : reaction.name),
  };
  if (options?.levels !== undefined) descriptor.levels = options.levels;
  return descriptor;
}

/** Identity builder for a trigger-pool declaration returned from a level or mod manifest. Engine parsing owns arming validation. */
export function defineTriggerPool(pool: TriggerPoolDescriptor): TriggerPoolDescriptor {
  return pool;
}
