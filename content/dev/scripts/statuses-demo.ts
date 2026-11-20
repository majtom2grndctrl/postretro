// The statuses Scripting surface example as one standalone mod manifest:
// two damage types, four statuses, an igniting policy, a cure and lava.
//
// content/dev's own manifest (start-script.ts) registers the same pieces,
// and is TypeScript only, so the Luau half of this example is a twin:
// statuses-demo.luau must produce the same manifest. Both run as mod
// manifests through `startup::status_content_tests` and drive the fixed tick
// in `sim::status_dev_fixture_tests`. No mod or map loads this file.
// See: docs/scripting-reference.md (Statuses)

import { defineMod } from "postretro";
import { acid, fireDamage } from "./damage-types";
import { burning, corroding, scalding, wet } from "./statuses";
import { cure, ignite, lava } from "./status-hazards";

export default defineMod({
  name: "statuses-demo",
  id: "example.statuses",
  version: "1",
  damageTypes: [fireDamage, acid],
  statuses: [burning, scalding, corroding, wet],
  fluids: [lava],
  events: [ignite],
  reactions: [cure],
});
