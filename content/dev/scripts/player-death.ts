// Author policy over the engine's player life-state floor.
// Luau twin: player-death.luau (named reactions keep the wire identical).
import {
  assistGiven,
  assistReceived,
  becomes,
  defineImpactEvent,
  defineReaction,
  getGameState,
  inLifeState,
  players,
  read,
  update,
} from "postretro";
import type { PlayerEventParams } from "postretro";
import { onStateCrossing, vignette } from "postretro/ui";
import { progression } from "./combat-lifecycle";

export const BLEED_OUT_MS = 15_000;
const player = getGameState().player;
const down = defineReaction("life.down", (on: PlayerEventParams) => on.player.setLifeState("down"));
const respawn = defineReaction("life.respawn", (on: PlayerEventParams) => on.player.respawn());
const downedVignette = defineReaction("life.downedVignette", vignette(0.7, BLEED_OUT_MS, [0.65, 0, 0]));
const clearVignette = defineReaction("life.clearVignette", vignette(0, 1, [0.65, 0, 0]));

// Borderlands revive: a teammate's completed assist stands the target up with
// 30 health and pays the helper; a kill made while down is a second wind.
const revived = defineReaction("life.revived", (on: PlayerEventParams) => [
  on.player.setLifeState("alive"),
  on.player.grantHealth(30),
]);
const assistBonus = defineReaction("life.assistBonus", (on: PlayerEventParams) =>
  on.player.addSlot(progression.xp, 25),
);

// Its own impact event, not a row in `enemyDeath`: the combat-demo zombie
// override evicts that base's rows where it applies.
export const secondWind = defineImpactEvent("second-wind", { tag: "enemy" }, (impact) => {
  const { target, source } = impact;
  const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
  return [
    {
      when: killed.and(source.inLifeState("down")),
      do: [
        source.setLifeState("alive"),
        source.grantHealth(40),
        update(progression.xp.byPlayer(source), (xp) => xp.plus(50)),
      ],
    },
  ];
});

export function setupPlayerDeath() {
  return {
    reactions: [down, respawn, downedVignette, clearVignette, revived, assistBonus],
    events: [secondWind],
    playerEvents: [
      // Not-down guard: the edge goes false while down, so lethal damage in the respawn tick still fires it.
      players().on(becomes(read(player.health).le(0).and(inLifeState("down").not())), [down]),
      players().on(becomes(inLifeState("down").and(read(player.lifeStateMs).ge(BLEED_OUT_MS))), [respawn]),
      players().on(assistReceived(), [revived]),
      players().on(assistGiven(), [assistBonus]),
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
