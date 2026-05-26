import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";

export function setupMod() {
  return {
    name: "dev",
    entities: [playerEntity, referencePistolEntity, ...referenceEntities],
  };
}
