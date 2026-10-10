// Per-player events: a guarded XP level-up and an overheat scald, each fired
// on the host once per player, with the fanfare and scald presenting on that
// player's own machine; a low-health vignette stays a local crossing.
// Luau twin: player-events.luau (byte-identical wire, so every reaction is
// named — derived ids differ per runtime).
// See: context/lib/scripting.md §12 (Player events)

import { players, becomes, ceases, read, defineReaction, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { playSound, flashScreen, vignette, updateState, onStateCrossing } from "postretro/ui";
import { progression } from "./combat-lifecycle";
import { leveling } from "./leveling";

const player = getGameState().player;

// One effect per reaction; a source's fire list runs several.
const levelUp = defineReaction("leveling.levelUp", (on: PlayerEventParams) =>
  on.player.addSlot(leveling.level, 1),
);
const recordLevelUp = defineReaction("leveling.recordLevelUp", (on: PlayerEventParams) =>
  updateState(leveling.lastLevelUpXp, read(progression.xp.byPlayer(on.player))),
);
const fanfare = defineReaction("leveling.fanfare", playSound("sfx/test_tone")); // on that player's machine
const goldFlash = defineReaction("leveling.goldFlash", flashScreen([1, 0.9, 0.3, 0.4], 300));
const scald = defineReaction("heat.scald", (on: PlayerEventParams) => on.player.damage(5));
const scaldHiss = defineReaction("heat.scaldHiss", playSound("fixtures/bite"));
const cooled = defineReaction("heat.cooled", playSound("movers/door_close"));
const bleeding = defineReaction("health.bleeding", vignette(0.6, 800, [0.8, 0, 0]));

export function setupLevel() {
  return {
    reactions: [levelUp, recordLevelUp, fanfare, goldFlash, scald, scaldHiss, cooled, bleeding],
    playerEvents: [
      // Host decisions: run on the host, once per player. The guard
      // (`level < 2`) goes false once the fire raises the level, so the
      // milestone never re-fires across a level change or a reclaim.
      players().on(becomes(read(progression.xp).ge(100).and(read(leveling.level).lt(2))), [
        levelUp,
        recordLevelUp,
        fanfare,
        goldFlash,
      ]),
      players().on(becomes(read(player.overheated)), [scald, scaldHiss]), // overheating burns the wielder
      players().on(ceases(read(player.overheated)), [cooled]),
    ],
    crossings: [
      // Own-state feedback: each machine, its own player.
      onStateCrossing(player.health, { below: 25 }, [bleeding]),
    ],
  };
}
