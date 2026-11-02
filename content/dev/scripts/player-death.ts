// Author policy over the engine's player life-state floor.
// Luau twin: player-death.luau (named reactions keep the wire identical).
import { becomes, defineReaction, getGameState, inLifeState, players, read } from "postretro";
import type { PlayerEventParams } from "postretro";
import { onStateCrossing, vignette } from "postretro/ui";

export const BLEED_OUT_MS = 15_000;
const player = getGameState().player;
const down = defineReaction("life.down", (on: PlayerEventParams) => on.player.setLifeState("down"));
const respawn = defineReaction("life.respawn", (on: PlayerEventParams) => on.player.respawn());
const downedVignette = defineReaction("life.downedVignette", vignette(0.7, BLEED_OUT_MS, [0.65, 0, 0]));
const clearVignette = defineReaction("life.clearVignette", vignette(0, 1, [0.65, 0, 0]));

export function setupPlayerDeath() {
  return {
    reactions: [down, respawn, downedVignette, clearVignette],
    playerEvents: [
      // Not-down guard: the edge goes false while down, so lethal damage in the respawn tick still fires it.
      players().on(becomes(read(player.health).le(0).and(inLifeState("down").not())), [down]),
      players().on(becomes(inLifeState("down").and(read(player.lifeStateMs).ge(BLEED_OUT_MS))), [respawn]),
    ],
    crossings: [
      onStateCrossing(inLifeState("down"), [downedVignette]),
      onStateCrossing(inLifeState("down").not(), [clearVignette]),
    ],
  };
}

// Standalone entry for the twin surface proof and focused content tools.
export function setupLevel() {
  return setupPlayerDeath();
}
