// Engine-owned state reference surface.
// The runtime installs a frozen reference tree under an internal bridge before
// this prelude runs. Capture it into a pure closure, then hide the bridge before
// author code evaluates.

const GAME_STATE_BRIDGE_GLOBAL = "__postretroGameStateRefs";

type GlobalWithBridge = typeof globalThis & Record<string, unknown>;

const gameStateRefs = (globalThis as GlobalWithBridge)[GAME_STATE_BRIDGE_GLOBAL];
if (gameStateRefs === undefined || gameStateRefs === null) {
  throw new Error("getGameState: missing engine state bridge");
}
delete (globalThis as GlobalWithBridge)[GAME_STATE_BRIDGE_GLOBAL];

export function getGameState(): import("postretro").GameStateRefs {
  return gameStateRefs as import("postretro").GameStateRefs;
}
