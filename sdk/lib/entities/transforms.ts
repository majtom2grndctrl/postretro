// Transform-only entity vocabulary: lightweight handle exposed by
// `world.query({ component: "transform" })`. Carries id + position + tags.
//
// Same shape as `Entity` from the generated SDK types — declared here as
// `TransformHandle` so the per-entity-type module structure stays consistent
// with `entities/lights.ts` / `entities/emitters.ts`.

import type { EntityId, Vec3 } from "postretro";

/** Handle returned by `world.query({ component: "transform" })`. */
export type TransformHandle = {
  id: EntityId;
  position: Vec3;
  tags: ReadonlyArray<string>;
};
