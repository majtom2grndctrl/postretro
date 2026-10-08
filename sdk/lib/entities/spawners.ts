// Spawner member handle. A spawner's archetype and count are authored on its
// map entity; `fire()` spawns one batch from this spawner only, when its step
// runs.

import type { EntityId, SpawnerEntity } from "postretro";
import type { SequenceStep } from "../data_script";

/** Spawner member returned by `getMapEntities("spawner")`. */
export interface SpawnerEntityHandle extends SpawnerEntity {
  /** Spawn one batch from this spawner, and from no sibling sharing its tag. */
  fire(): SequenceStep[];
}

export function wrapSpawnerEntity(snapshot: SpawnerEntity): SpawnerEntityHandle {
  const id: EntityId = snapshot.id;
  return {
    ...snapshot,
    fire(): SequenceStep[] {
      return [{ id, primitive: "spawnFromSpawner" }];
    },
  };
}
