// Characters, runs and checkpoints scripting-surface fixture: the brief's
// Scripting surface example (coop--character-and-run-saves) made runnable, as
// one standalone mod manifest. A world slot, a run-scoped slot and a
// per-owner character slot; a run row with CONTINUE, NEW GAME and DELETE per
// slot; a character row with SELECT, CREATE and DELETE per slot; and the
// checkpoint a level script would fire.
//
// saves-surface-fixture.luau is its Luau twin: both must produce the same
// manifest, and `app::saves_surface_fixture_tests` runs each with identity
// enforcement on. The persisted slots are minted in content/dev/identity.json
// under namespaces the dev mod never declares. No mod or map loads this file.
// See: docs/scripting-reference.md (Characters, runs and checkpoints)

import { defineMapCatalog, defineMod, defineReaction, defineStore } from "postretro";
import {
  Button,
  HStack,
  Text,
  Tree,
  VStack,
  bindState,
  checkpoint,
  continueRun,
  createCharacter,
  defineUiTree,
  deleteCharacter,
  deleteRun,
  forEach,
  getGameState,
  newRun,
  openTextEntry,
  selectCharacter,
  stateEquals,
} from "postretro/ui";

const meta = defineStore("savesFixture.meta", {
  episode2: { type: "boolean", default: false, persist: true }, // world file
});
const run = defineStore("savesFixture.run", {
  generatorOn: { type: "boolean", default: false, persist: "run" }, // run slot
});
const hero = defineStore("savesFixture.hero", {
  xp: { type: "number", default: 0, perOwner: true, persist: true }, // character slot
});

const saves = getGameState().saves;
const CHARACTER_SLOTS = 4;
const RUN_SLOTS = 3;

// Reaction names count rows from 0, as TypeScript's `forEach` does.
const runReactions = Array.from({ length: RUN_SLOTS }, (_, i) => [
  defineReaction(`runs.continue.${i}`, continueRun(i)),
  defineReaction(`runs.new.${i}`, newRun(i, "campaign-test")),
  defineReaction(`runs.delete.${i}`, deleteRun(i)),
]).flat();
const runRows = forEach(saves.runs, (row, i) =>
  HStack({}, [
    Text({ content: "EMPTY", bind: bindState(row.levelName) }),
    Text({ content: "", bind: bindState(row.characterName) }),
    Button({
      id: "continue",
      label: "CONTINUE",
      onPress: `runs.continue.${i}`,
      visibleWhen: stateEquals(row.occupied, true),
    }),
    Button({ id: "new", label: "NEW GAME", onPress: `runs.new.${i}` }),
    Button({ id: "delete", label: "DELETE", onPress: `runs.delete.${i}` }),
  ]),
);

// CREATE opens the keyboard on `ui.textEntry`; its commit creates the
// character, which takes that text as its name.
const characterReactions = Array.from({ length: CHARACTER_SLOTS }, (_, i) => [
  defineReaction(`characters.select.${i}`, selectCharacter(i)),
  defineReaction(`characters.name.${i}`, openTextEntry(`characters.create.${i}`)),
  defineReaction(`characters.create.${i}`, createCharacter(i)),
  defineReaction(`characters.delete.${i}`, deleteCharacter(i)),
]).flat();
const characterRows = forEach(saves.characters, (row, i) =>
  HStack({}, [
    Text({ content: "", bind: bindState(row.name) }),
    Button({
      id: "select",
      label: "SELECT",
      onPress: `characters.select.${i}`,
      visibleWhen: stateEquals(row.readable, true),
    }),
    Button({
      id: "create",
      label: "CREATE",
      onPress: `characters.name.${i}`,
      visibleWhen: stateEquals(row.occupied, false),
    }),
    Button({ id: "delete", label: "DELETE", onPress: `characters.delete.${i}` }),
  ]),
);

// In a level script, the host saves a checkpoint; its player_spawns carry checkpoint=bridge.
const reachBridge = defineReaction("level.bridge", checkpoint("bridge"));

const savesScreen = defineUiTree({
  name: "savesSurfaceFixture",
  tree: Tree(
    { anchor: "center", offset: [0, 0], captureMode: "capture", initialFocus: "continue.0" },
    VStack({ gap: 8, focus: { policy: "linear", wrap: true } }, [...characterRows, ...runRows]),
  ),
});

export default defineMod({
  name: "saves-surface-fixture",
  id: "postretro.dev-saves-surface",
  version: "dev",
  saves: { characterSlots: CHARACTER_SLOTS, runSlots: RUN_SLOTS },
  stores: [meta, run, hero],
  reactions: [...runReactions, ...characterReactions, reachBridge],
  maps: defineMapCatalog([{ id: "campaign-test", path: "maps/campaign-test.prl", name: "Moody Vibe Test" }]),
  uiTrees: [savesScreen],
});
