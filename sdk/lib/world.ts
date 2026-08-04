// World-query vocabulary: typed wrapper around `worldQuery`. Light,
// fog-volume, mover, and trigger handle construction delegate to
// `./entities/*`.

import { worldQuery, worldGetGravity, worldSetGravity } from "postretro";
import type {
  EmitterEntity,
  Entity,
  FogVolumeEntity as GeneratedFogVolumeEntity,
  LightEntity as GeneratedLightEntity,
  MoverEntity as GeneratedMoverEntity,
  TriggerVolumeEntity as GeneratedTriggerVolumeEntity,
  WorldQueryFilter,
} from "postretro";
import { wrapLightEntity } from "./entities/lights";
import type { LightEntityHandle } from "./entities/lights";
import { wrapFogVolumeEntity } from "./entities/fog_volumes";
import type { FogVolumeHandle } from "./entities/fog_volumes";
import { wrapMoverEntity } from "./entities/movers";
import type { MoverEntityHandle } from "./entities/movers";
import { wrapTriggerVolumeEntity } from "./entities/triggers";
import type { TriggerVolumeHandle } from "./entities/triggers";

/**
 * Extend this as new component types gain dedicated handles; unknown
 * component names fall back to `Entity`. `"emitter"` yields a handle
 * carrying the full `BillboardEmitterComponent` snapshot under `component`.
 */
export type EntityForComponent<T extends string> =
  T extends "light" ? LightEntityHandle :
  T extends "emitter" ? EmitterEntity :
  T extends "fog_volume" ? FogVolumeHandle :
  T extends "kinematic_mover" ? MoverEntityHandle :
  T extends "trigger_volume" ? TriggerVolumeHandle :
  Entity;

/** Typed vocabulary object returned from `world.query`. */
export interface World {
  /**
   * Query entities matching the filter. The return type is selected by
   * the literal `component` string: `"light"` yields `LightEntityHandle[]`
   * (carrying `pulse` / `fade` / `flicker` / `colorShift` / `sweep`
   * capability methods); `"fog_volume"` yields `FogVolumeHandle[]` (carrying
   * `pulse` / `fade` / `flicker` / `pulseSaturation` / `fadeSaturation`
   * capability methods); `"kinematic_mover"` yields `MoverEntityHandle[]`
   * (carrying start / stop / reverse / goToPathNode / setSpinRate / setBlockPolicy command builders);
   * `"trigger_volume"` yields `TriggerVolumeHandle[]` (carrying arm / disarm
   * command builders); `"emitter"` yields handles carrying the full
   * `BillboardEmitterComponent` snapshot under `component`; any other
   * component name yields base `Entity[]` (id, position, tags).
   *
   * Supported component strings: `"light"`, `"fog_volume"`, `"kinematic_mover"`, `"trigger_volume"`, `"transform"`,
   * `"emitter"`, `"particle"`, `"sprite_visual"`. Note that `"particle"` and
   * `"sprite_visual"` always return `[]` (engine-managed; scripts never
   * iterate individual particles or sprite visuals). Unknown component
   * strings throw `InvalidArgument` at runtime.
   */
  query<T extends string>(
    filter: { component: T; tag?: string | null },
  ): EntityForComponent<T>[];

  /**
   * Current world gravity in m/s². Negative = downward (Earth = -9.81),
   * positive = upward. Seeded from the worldspawn `initialGravity` KVP at
   * level load and persists until the next level load or `setGravity` call.
   */
  getGravity(): number;

  /**
   * Set the world gravity in m/s². Negative = downward, positive = upward.
   * NaN and non-finite values are silently ignored (a warning is logged).
   * Effect is immediate and persists until the next level load or another
   * `setGravity` call.
   */
  setGravity(value: number): void;
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
    if (filter.component === "fog_volume") {
      const volumes = (raw as ReadonlyArray<GeneratedFogVolumeEntity>).map(
        wrapFogVolumeEntity,
      );
      return volumes as EntityForComponent<T>[];
    }
    if (filter.component === "kinematic_mover") {
      const movers = (raw as ReadonlyArray<GeneratedMoverEntity>).map(
        wrapMoverEntity,
      );
      return movers as EntityForComponent<T>[];
    }
    if (filter.component === "trigger_volume") {
      const triggers = (raw as ReadonlyArray<GeneratedTriggerVolumeEntity>).map(
        wrapTriggerVolumeEntity,
      );
      return triggers as EntityForComponent<T>[];
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
  getGravity(): number {
    return worldGetGravity();
  },
  setGravity(value: number): void {
    worldSetGravity(value);
  },
};
