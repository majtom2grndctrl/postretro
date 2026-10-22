// Projectile trap member handle. A trap's weapon, interval and phase are
// authored on its map entity; `fire()`, `start()` and `stop()` return steps
// that act on the host only, when they run.
// See: context/lib/scripting.md §12 (Entity addressing)

import type { EntityId, TrapEntity, TrapStartOptions } from "postretro";
import type { SequenceStep } from "../data_script";

/** Projectile trap member returned by `getMapEntities("trap")`. */
export interface TrapEntityHandle extends TrapEntity {
  /** Run one execution of the trap's weapon. Dropped while an execution or its recovery runs. */
  fire(): SequenceStep[];
  /** Start repeating at `intervalMs` after `phaseMs`. A no-op on a running trap. Options apply to this run only; an omitted field takes the authored value. */
  start(options?: TrapStartOptions): SequenceStep[];
  /** Cancel future repeats; an execution already running finishes. */
  stop(): SequenceStep[];
}

export function wrapTrapEntity(snapshot: TrapEntity): TrapEntityHandle {
  const id: EntityId = snapshot.id;
  return {
    ...snapshot,
    fire(): SequenceStep[] {
      return [{ id, primitive: "fireTrap" }];
    },
    start(options?: TrapStartOptions): SequenceStep[] {
      // Only the fields the author named reach the wire: an omitted one takes
      // the trap's authored value, while an explicit bad value is rejected.
      const args: TrapStartOptions = {};
      if (options?.intervalMs !== undefined) args.intervalMs = options.intervalMs;
      if (options?.phaseMs !== undefined) args.phaseMs = options.phaseMs;
      return [{ id, primitive: "startTrap", args }];
    },
    stop(): SequenceStep[] {
      return [{ id, primitive: "stopTrap" }];
    },
  };
}
