// Area lights: `"Area"` is a `LightKind`, and an area light is an ordinary
// light member with the light verbs.
// See: context/lib/scripting.md §12 (Entity addressing)
import { type LightKind, defineReaction, getMapEntities } from "postretro";

const area: LightKind = "Area";
const kinds: LightKind[] = ["Point", "Spot", "Directional", area];
// @ts-expect-error -- not a light kind
const rect: LightKind = "Rect";

export function setupLevel(_ctx: unknown) {
  const panels = getMapEntities("light", { tag: "skylight" });
  const areas = panels.filter((p) => p.component.lightType === "Area");
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: areas.flatMap((p) => p.pulse({ min: 0.3, max: 1.0, periodMs: 900 })),
      }),
    ],
  };
}

void kinds;
void rect;
