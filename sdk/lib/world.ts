// World-query vocabulary: typed wrapper around `worldQuery`. Light handle
// construction delegates to `./entities/lights`.

import { worldQuery } from "postretro";
import type {
  Entity,
  LightEntity as GeneratedLightEntity,
  WorldQueryFilter,
} from "postretro";
import { wrapLightEntity } from "./entities/lights";
import type { LightEntity } from "./entities/lights";

/**
 * Maps a component-name string literal to the rich entity handle type
 * returned by `world.query`. Extend this as new component types gain
 * dedicated handles; unknown component names fall back to `Entity`.
 */
export type EntityForComponent<T extends string> =
  T extends "light" ? LightEntity : Entity;

/** Typed vocabulary object returned from `world.query`. */
export interface World {
  /**
   * Query entities matching the filter. The return type is selected by
   * the literal `component` string: `"light"` yields `LightEntity[]`
   * (with convenience methods `setAnimation`, `setIntensity`,
   * `setColor`); any other component name yields base `Entity[]`
   * (id, transform, tags) — use `getComponent` to access component data.
   *
   * **Note:** Unknown `component` strings (e.g. typos like `"lights"`)
   * compile without error but throw `InvalidArgument` at runtime; only
   * `"light"` is supported in the current build.
   */
  query<T extends string>(
    filter: { component: T; tag?: string | null },
  ): EntityForComponent<T>[];
}

export const world: World = {
  query<T extends string>(
    filter: { component: T; tag?: string | null },
  ): EntityForComponent<T>[] {
    const normalized: WorldQueryFilter = {
      component: filter.component,
      tag: filter.tag ?? null,
    };
    const raw = worldQuery(normalized);
    if (filter.component === "light") {
      const lights = (raw as ReadonlyArray<GeneratedLightEntity>).map(
        wrapLightEntity,
      );
      return lights as EntityForComponent<T>[];
    }
    // Project per-component snapshots down to the `Entity` shape so
    // callers using the generic path don't observe component-specific
    // fields that the type does not promise.
    const entities: Entity[] = raw.map((s) => ({
      id: s.id,
      transform: s.transform,
      tags: s.tags,
    }));
    return entities as EntityForComponent<T>[];
  },
};
