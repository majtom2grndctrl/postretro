// The mod's King-of-the-Hill store: control drains by one per second for each
// player standing on any volume tagged `hill`, on whichever level has one. A
// mod-scope `occupancy` reference: levels without a hill read 0 silently.
// See: context/lib/scripting.md §5 (Occupancy)

import { defineStore, getGameState, runtime } from "postretro";

const { occupancy } = getGameState();

export const kingHill = defineStore("kingHill", {
  control: {
    type: "number",
    default: 100,
    range: [0, 100],
    network: "shared",
    accumulate: (t) =>
      runtime.mul(t.dt, runtime.mul(runtime.read(occupancy.tag("hill").occupants), -1)),
  },
});
