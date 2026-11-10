// Attack pool member handle. A pool's member tag and budget profile are
// authored on its `ai_attack_pool` map entity; `start()` and `stop()` return
// steps that act on the host only, when they run.
// See: context/lib/scripting.md §12 (Entity addressing)

import type { AttackPoolEntity, EntityId } from "postretro";
import type { SequenceStep } from "../data_script";

/** Attack pool member returned by `getMapEntities("attackPool")`. */
export interface AttackPoolEntityHandle extends AttackPoolEntity {
  /** Start the pool: every enemy carrying its member tag takes turns in it, under its budget profile. On a running pool, re-assigns its current tag holders; turns, waits and refreshing counts stand. */
  start(): SequenceStep[];
  /** Stop the pool: its members return to the default pool and its refreshing counts drop. A no-op on a stopped pool. */
  stop(): SequenceStep[];
}

export function wrapAttackPoolEntity(snapshot: AttackPoolEntity): AttackPoolEntityHandle {
  const id: EntityId = snapshot.id;
  return {
    ...snapshot,
    start(): SequenceStep[] {
      return [{ id, primitive: "startAttackPool" }];
    },
    stop(): SequenceStep[] {
      return [{ id, primitive: "stopAttackPool" }];
    },
  };
}
