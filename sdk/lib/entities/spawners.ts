// Spawner vocabulary: fire-time tag-targeted consequence descriptors.
// A spawner's archetype and count are authored on the map entity; firing only
// addresses the live spawner group when the reaction dispatches.

import type { PrimitiveReactionDescriptor } from "../data_script";

/** Selects the live spawner group addressed when a reaction fires. */
export type SpawnerFilter = {
  tag?: string;
};

/** Fire-time-tag spawner handle. Methods emit one primitive reaction descriptor. */
export interface SpawnerHandle {
  fire(): PrimitiveReactionDescriptor;
}

/** Select a live spawner group by tag and fire each matching spawner. */
export function spawner(filter: SpawnerFilter): SpawnerHandle {
  return {
    fire() {
      return { primitive: "spawnFromSpawner", tag: filter.tag };
    },
  };
}
