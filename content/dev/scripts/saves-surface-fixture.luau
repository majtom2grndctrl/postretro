-- Luau twin of saves-surface-fixture.ts: the characters, runs and checkpoints
-- scripting-surface example as a standalone mod manifest. Both must produce
-- the same manifest; `app::saves_surface_fixture_tests` runs each with
-- identity enforcement on. Luau's `forEach` and loops count rows from 1, and
-- the save verbs take that index, so `UI.continueRun(i)` here crosses as the
-- TypeScript twin's `continueRun(i - 1)`. Reaction names stay 0-based in both,
-- so each row's press names the same reaction. No mod or map loads this file.
-- See: docs/scripting-reference.md (Characters, runs and checkpoints)

local Postretro = require("postretro")
local UI = require("postretro/ui")

local meta = Postretro.defineStore("savesFixture.meta", {
  episode2 = { type = "boolean", default = false, persist = true }, -- world file
})
local run = Postretro.defineStore("savesFixture.run", {
  generatorOn = { type = "boolean", default = false, persist = "run" }, -- run slot
})
local hero = Postretro.defineStore("savesFixture.hero", {
  xp = { type = "number", default = 0, perOwner = true, persist = true }, -- character slot
})

local saves = UI.getGameState().saves
local CHARACTER_SLOTS = 4
local RUN_SLOTS = 3

local reactions = {}

for i = 1, RUN_SLOTS do
  local row = i - 1
  table.insert(reactions, Postretro.defineReaction(`runs.continue.{row}`, UI.continueRun(i)))
  table.insert(reactions, Postretro.defineReaction(`runs.new.{row}`, UI.newRun(i, "campaign-test")))
  table.insert(reactions, Postretro.defineReaction(`runs.delete.{row}`, UI.deleteRun(i)))
end
local runRows = UI.forEach(saves.runs, function(row, i)
  return UI.HStack({}, {
    UI.Text({ content = "EMPTY", bind = UI.bindState(row.levelName) }),
    UI.Text({ content = "", bind = UI.bindState(row.characterName) }),
    UI.Button({
      id = "continue",
      label = "CONTINUE",
      onPress = `runs.continue.{i - 1}`,
      visibleWhen = UI.stateEquals(row.occupied, true),
    }),
    UI.Button({ id = "new", label = "NEW GAME", onPress = `runs.new.{i - 1}` }),
    UI.Button({ id = "delete", label = "DELETE", onPress = `runs.delete.{i - 1}` }),
  })
end)

-- CREATE opens the keyboard on `ui.textEntry`; its commit creates the
-- character, which takes that text as its name.
for i = 1, CHARACTER_SLOTS do
  local row = i - 1
  table.insert(reactions, Postretro.defineReaction(`characters.select.{row}`, UI.selectCharacter(i)))
  table.insert(
    reactions,
    Postretro.defineReaction(`characters.name.{row}`, UI.openTextEntry(`characters.create.{row}`))
  )
  table.insert(reactions, Postretro.defineReaction(`characters.create.{row}`, UI.createCharacter(i)))
  table.insert(reactions, Postretro.defineReaction(`characters.delete.{row}`, UI.deleteCharacter(i)))
end
local characterRows = UI.forEach(saves.characters, function(row, i)
  return UI.HStack({}, {
    UI.Text({ content = "", bind = UI.bindState(row.name) }),
    UI.Button({
      id = "select",
      label = "SELECT",
      onPress = `characters.select.{i - 1}`,
      visibleWhen = UI.stateEquals(row.readable, true),
    }),
    UI.Button({
      id = "create",
      label = "CREATE",
      onPress = `characters.name.{i - 1}`,
      visibleWhen = UI.stateEquals(row.occupied, false),
    }),
    UI.Button({ id = "delete", label = "DELETE", onPress = `characters.delete.{i - 1}` }),
  })
end)

-- In a level script, the host saves a checkpoint; its player_spawns carry checkpoint=bridge.
table.insert(reactions, Postretro.defineReaction("level.bridge", UI.checkpoint("bridge")))

local rows = {}
for _, widget in characterRows do
  table.insert(rows, widget)
end
for _, widget in runRows do
  table.insert(rows, widget)
end

local savesScreen = UI.defineUiTree({
  name = "savesSurfaceFixture",
  tree = UI.Tree(
    { anchor = "center", offset = { 0, 0 }, captureMode = "capture", initialFocus = "continue.0" },
    UI.VStack({ gap = 8, focus = { policy = "linear", wrap = true } }, rows)
  ),
})

return Postretro.defineMod({
  name = "saves-surface-fixture",
  id = "postretro.dev-saves-surface",
  version = "dev",
  saves = { characterSlots = CHARACTER_SLOTS, runSlots = RUN_SLOTS },
  stores = { meta, run, hero },
  reactions = reactions,
  maps = Postretro.defineMapCatalog({
    { id = "campaign-test", path = "maps/campaign-test.prl", name = "Moody Vibe Test" },
  }),
  uiTrees = { savesScreen },
})
