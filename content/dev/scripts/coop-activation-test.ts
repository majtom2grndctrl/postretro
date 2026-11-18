// The E18 co-op activation dev map: two vault plates held together open the
// vault door, the airlock opens once every player in play stands inside, and
// the hill (the mod's `kingHill` store) drains while anyone holds it. A HUD
// strip shows how many players stand in the airlock.
// See: context/lib/scripting.md §5 (Session players, Occupancy)

import { defineReaction, getGameState, getMapEntities, runtime } from "postretro";
import type { NamedReactionDescriptor } from "postretro";
import { Text, Tree, bindState, defineUiTree, onStateCrossing } from "postretro/ui";
import type { UiTreeRegistration } from "postretro/ui";

const { session, occupancy } = getGameState();
const counts = session.playerCounts;
// Read at module scope, which the build-time bridge evaluates as install does.
const vaultPlates = occupancy.tag("vault-plates");

export function setupLevel(): {
  reactions: NamedReactionDescriptor[];
  crossings: unknown[];
  uiTrees: UiTreeRegistration[];
} {
  const vaultDoor = getMapEntities("mover", { tag: "vault-door" });
  const airlockDoor = getMapEntities("mover", { tag: "airlock-door" });
  const solve = defineReaction("vault.solve", { sequence: vaultDoor.flatMap((d) => d.start()) });
  const allIn = defineReaction("airlock.allIn", { sequence: airlockDoor.flatMap((d) => d.start()) });

  // The room's tag comes from the map, not a literal: a tag built at run time
  // still gets its occupancy family.
  const roomTag =
    getMapEntities("trigger")
      .flatMap((volume) => volume.tags)
      .find((tag) => tag.startsWith("air")) ?? "airlock";
  const room = runtime.read(occupancy.tag(roomTag).occupants);
  const inPlay = runtime.read(counts.inPlay);

  return {
    reactions: [solve, allIn],
    crossings: [
      onStateCrossing(vaultPlates.occupiedCount, { above: 1 }, [solve]),
      // Everyone is in: nobody loading, someone in play, all of them inside.
      onStateCrossing(
        runtime.and(
          runtime.eq(runtime.read(counts.loading), 0),
          runtime.and(runtime.gt(inPlay, 0), runtime.ge(room, inPlay)),
        ),
        [allIn],
      ),
    ],
    uiTrees: [
      defineUiTree({
        name: "coop.airlock",
        alwaysOn: true,
        tree: Tree(
          { anchor: "top", offset: [0, 24] },
          Text({
            content: "",
            fontSize: 18,
            bind: bindState(occupancy.tag(roomTag).occupants, { format: "IN THE AIRLOCK {}" }),
          }),
        ),
      }),
    ],
  };
}
