// World-query vocabulary: typed wrapper around `worldQuery`. Light handle
// construction delegates to `./entities/lights`.

import { worldQuery } from "postretro";
import type {
  EmitterEntity,
  Entity,
  LightEntity as GeneratedLightEntity,
  WorldQueryFilter,
} from "postretro";
import { wrapLightEntity } from "./entities/lights";
import type { LightEntity } from "./entities/lights";

/**
 * Extend this as new component types gain dedicated handles; unknown
 * component names fall back to `Entity`. `"emitter"` yields a handle
 * carrying the full `BillboardEmitterComponent` snapshot under
 * `component` — use it to read live emitter fields without a follow-up
 * `getComponent` call.
 */
export type EntityForComponent<T extends string> =
  T extends "light" ? LightEntity :
  T extends "emitter" ? EmitterEntity :
  Entity;

/** Typed vocabulary object returned from `world.query`. */
export interface World {
  /**
   * Query entities matching the filter. The return type is selected by
   * the literal `component` string: `"light"` yields `LightEntity[]`
   * (with convenience methods `setAnimation`, `setIntensity`,
   * `setColor`); any other component name yields base `Entity[]`
   * (id, position, tags) — use `getComponent` to access component data.
   *
   * Supported component strings: `"light"`, `"transform"`, `"emitter"`,
   * `"particle"`, `"sprite_visual"`. Note that `"particle"` and
   * `"sprite_visual"` always return `[]` (engine-managed; scripts never
   * iterate individual particles or sprite visuals). Unknown component
   * strings throw `InvalidArgument` at runtime.
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
    // Thread the optional `component` sub-object through unchanged for
    // queries (e.g. `"emitter"`) whose Rust handle carries it; queries
    // whose Rust handle omits `component` (e.g. `"transform"`) leave it
    // undefined on the projected handle, matching the bare `Entity` type.
    const entities = raw.map((s) => {
      const projected: Entity & { component?: unknown } = {
        id: s.id,
        position: s.position,
        tags: s.tags,
      };
      if ((s as { component?: unknown }).component !== undefined) {
        projected.component = (s as { component: unknown }).component;
      }
      return projected;
    });
    return entities as EntityForComponent<T>[];
  },
};
