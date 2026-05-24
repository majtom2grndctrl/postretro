import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { arenaLightEntities } from "./scripts/arena-lights";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";

export function setupMod() {
  return {
    name: "dev",
    entities: [playerEntity, referencePistolEntity, ...arenaLightEntities, ...referenceEntities],
  };
}
