// DEV FIXTURE — world marker over each teammate who can be revived.
//
// The engine decides which teammates are marked and sets `inRange` on the one
// a held `assist` would revive; this file only decides how the marker looks.
// The dev player model has no `head` socket, so `offsetY` alone lifts the mark
// to eye level above the pawn's origin.

import {
  Glyph,
  HStack,
  Text,
  VStack,
  assistableTeammates,
  defineOverlay,
  definePresentationTemplate,
  fact,
} from "postretro/ui";

export const reviveMarker = definePresentationTemplate({
  root: VStack({ gap: 2.0 }, [
    Text({ content: "DOWNED", fontSize: 14.0, color: [1.0, 0.3, 0.3, 1.0] }),
    HStack({ gap: 4.0, visibleWhen: fact.bool("inRange") }, [
      Text({ content: "Hold", fontSize: 14.0, color: [1.0, 1.0, 1.0, 1.0] }),
      Glyph({ command: "assist" }),
      Text({ content: "to revive", fontSize: 14.0, color: [1.0, 1.0, 1.0, 1.0] }),
    ]),
  ]),
  lifetimeMs: 1,
  motion: { rise: 0.0, easing: "linear" },
  fade: { startMs: 1 },
  spawnScatter: { radius: 0.0 },
  worldAnchor: { offsetY: 0.6 },
});

export const reviveMarkerOverlay = defineOverlay({
  over: assistableTeammates(),
  template: reviveMarker,
  maxVisible: 8,
});
