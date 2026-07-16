// Enemy-group vocabulary: fire-time tag-targeted consequence descriptors.
// Unlike world.query handles, an EnemyGroup resolves its tag when the reaction
// fires, so enemies spawned after level install are still included.

import type { PrimitiveReactionDescriptor } from "../data_script";

/** Selects the live enemy group addressed when a reaction fires. */
export type EnemyGroupFilter = {
  tag?: string;
};

/** Typed, additive partial for consequential enemy-state updates. */
export type EnemyStateUpdateArgs = {
  aggro?: boolean;
};

/** Fire-time-tag enemy handle. Methods emit one primitive reaction descriptor. */
export interface EnemyGroup {
  update(fields: EnemyStateUpdateArgs): PrimitiveReactionDescriptor;
}

/**
 * Select a live enemy group by tag. The returned handle resolves its tag at
 * reaction fire time rather than at level-install query time.
 */
export function enemies(filter: EnemyGroupFilter): EnemyGroup {
  return {
    update(fields) {
      return { primitive: "updateEnemyState", tag: filter.tag, args: fields };
    },
  };
}
