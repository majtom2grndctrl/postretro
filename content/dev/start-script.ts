import { playerEntity } from "./scripts/player";
import { arenaLightEntities } from "./scripts/arena-lights";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";

export function setupMod() {
  return {
    name: "dev",
    entities: [playerEntity, ...arenaLightEntities, ...referenceEntities],
  };
}
