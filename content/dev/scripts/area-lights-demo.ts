// Area lights: the Scripting-surface example, run on `area-lights-demo.map`.
// Both ceiling skylights are `light_area` panels tagged `skylight` with no
// `_animated` key; pulsing them from `levelLoad` reserves their animated bake,
// and their glass glows with the room. An area light is an ordinary light
// member: `lightType` reads "Area", `position` is its emitting faces'
// area-weighted centroid, and brightness and colour animate. `sweep` on one
// fails with InvalidArgument, since an area light has no direction.
//
// The Luau twin, `area-lights-demo.luau`, emits identical wire.
// See: context/lib/scripting.md §12 (Entity addressing)

import { defineReaction, getMapEntities } from "postretro";

export function setupLevel(_ctx: unknown) {
  const panels = getMapEntities("light", { tag: "skylight" });
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: panels.flatMap((p) => p.pulse({ min: 0.3, max: 1.0, periodMs: 900 })),
      }),
    ],
  };
}
// p.component.lightType === "Area"; p.position is the emitting faces' centroid.
// p.sweep(...) on an area light fails with InvalidArgument.
