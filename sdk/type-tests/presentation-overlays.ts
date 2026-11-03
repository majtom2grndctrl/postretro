import type { ModManifest } from "postretro";
import { defineMod } from "postretro";
import {
  Glyph, HStack, Text, VStack, assistableTeammates, damagedEnemies, defineOverlay,
  definePresentationTemplate, fact,
} from "postretro/ui";

const reviveMarker = definePresentationTemplate({
  root: VStack({ gap: 2 }, [
    Text({ content: "DOWNED", fontSize: 14, color: [1, 0.3, 0.3, 1] }),
    HStack({ gap: 4, visibleWhen: fact.bool("inRange") }, [
      Text({ content: "Hold", fontSize: 14, color: [1, 1, 1, 1] }),
      Glyph({ command: "assist" }),
      Text({ content: "to revive", fontSize: 14, color: [1, 1, 1, 1] }),
    ]),
  ]),
  lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
  spawnScatter: { radius: 0 },
  worldAnchor: { offsetY: 0.6 },
});
const enemyStatus = definePresentationTemplate({
  root: Text({ content: "enemy", fontSize: 12 }),
  lifetimeMs: 1, motion: { rise: 0, easing: "linear" }, fade: { startMs: 1 },
  spawnScatter: { radius: 0 },
  worldAnchor: { socket: "head", offsetY: 0.35 },
});

const reviveMarkerOverlay = defineOverlay({ over: assistableTeammates(), template: reviveMarker, maxVisible: 8 });
const damagedEnemyOverlay = defineOverlay({
  over: damagedEnemies({ lingerMs: 500, hideAtFull: true }), template: enemyStatus, maxVisible: 4,
});

const single: ModManifest = defineMod({
  name: "single", id: "single", version: "1",
  presentationTemplates: [enemyStatus],
  presentationOverlays: damagedEnemyOverlay,
});
const both: ModManifest = defineMod({
  name: "both", id: "both", version: "1",
  presentationTemplates: [reviveMarker, enemyStatus],
  presentationOverlays: [damagedEnemyOverlay, reviveMarkerOverlay],
});
// @ts-expect-error The marker source takes no options.
const withOptions = assistableTeammates({ radius: 2 });

export { both, single, withOptions };
