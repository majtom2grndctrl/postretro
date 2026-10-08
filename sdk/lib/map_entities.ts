// Map members: `getMapEntities(kind, { tag? })` returns the map-placed
// instances of one kind, each snapshot wrapped in that kind's handle
// (`./entities/*`). Membership is fixed at install, so these are plain arrays
// the author inspects in JS.
// See: context/lib/scripting.md §12 (Entity addressing)

import type {
  EmitterEntity,
  FogVolumeEntity,
  LightEntity,
  MoverEntity,
  SpawnerEntity,
  TriggerVolumeEntity,
} from "postretro";
import { wrapLightEntity } from "./entities/lights";
import type { LightEntityHandle } from "./entities/lights";
import { wrapFogVolumeEntity } from "./entities/fog_volumes";
import type { FogVolumeHandle } from "./entities/fog_volumes";
import { wrapMoverEntity } from "./entities/movers";
import type { MoverEntityHandle } from "./entities/movers";
import { wrapTriggerVolumeEntity } from "./entities/triggers";
import type { TriggerVolumeHandle } from "./entities/triggers";
import { wrapSpawnerEntity } from "./entities/spawners";
import type { SpawnerEntityHandle } from "./entities/spawners";

// The raw engine query stays out of the author-facing typedefs: its kindless
// component spelling is what `getMapEntities` replaces. The runtime installs
// it as a global; this declaration is type-only and strips at bundle time.
declare function worldQuery(filter: { component: string; tag: string | null }): ReadonlyArray<unknown>;

/** Map-placed kinds `getMapEntities` accepts. NPCs and players are groups (`npcs`, `players`), not members. */
export type MapEntityKind = "mover" | "trigger" | "light" | "fog" | "emitter" | "spawner";

/** The member handle each map kind yields. */
export type MapEntityForKind<K extends MapEntityKind> =
  K extends "mover" ? MoverEntityHandle :
  K extends "trigger" ? TriggerVolumeHandle :
  K extends "light" ? LightEntityHandle :
  K extends "fog" ? FogVolumeHandle :
  K extends "emitter" ? EmitterEntity :
  K extends "spawner" ? SpawnerEntityHandle :
  never;

/** Narrows a member query to instances carrying `tag`. */
export type MapEntityFilter = { tag?: string };

// Each map kind's engine component name.
const MAP_KIND_COMPONENTS: Readonly<Record<MapEntityKind, string>> = Object.freeze({
  mover: "kinematic_mover",
  trigger: "trigger_volume",
  light: "light",
  fog: "fog_volume",
  emitter: "emitter",
  spawner: "spawner",
});

function projectEmitter(snapshot: EmitterEntity): EmitterEntity {
  return {
    id: snapshot.id,
    position: snapshot.position,
    tags: snapshot.tags,
    component: snapshot.component,
  };
}

/**
 * Return the map-placed members of `kind`, optionally only those carrying
 * `tag`, in authored map order. An instance a runtime spawn carries never
 * appears. Returns `[]` on no match. Callable only inside a level's
 * `setupLevel`; elsewhere it raises.
 */
export function getMapEntities<K extends MapEntityKind>(
  kind: K,
  filter?: MapEntityFilter,
): MapEntityForKind<K>[] {
  if (!Object.prototype.hasOwnProperty.call(MAP_KIND_COMPONENTS, kind)) {
    throw new TypeError(
      `getMapEntities: unknown kind ${JSON.stringify(kind)}; expected one of ` +
        Object.keys(MAP_KIND_COMPONENTS).map((k) => JSON.stringify(k)).join(", "),
    );
  }
  const raw = worldQuery({ component: MAP_KIND_COMPONENTS[kind], tag: filter?.tag ?? null });
  const members: unknown[] = (() => {
    switch (kind as MapEntityKind) {
      case "mover":
        return (raw as ReadonlyArray<MoverEntity>).map(wrapMoverEntity);
      case "trigger":
        return (raw as ReadonlyArray<TriggerVolumeEntity>).map(wrapTriggerVolumeEntity);
      case "light":
        return (raw as ReadonlyArray<LightEntity>).map(wrapLightEntity);
      case "fog":
        return (raw as ReadonlyArray<FogVolumeEntity>).map(wrapFogVolumeEntity);
      case "emitter":
        return (raw as ReadonlyArray<EmitterEntity>).map(projectEmitter);
      case "spawner":
        return (raw as ReadonlyArray<SpawnerEntity>).map(wrapSpawnerEntity);
    }
  })();
  return members as MapEntityForKind<K>[];
}
