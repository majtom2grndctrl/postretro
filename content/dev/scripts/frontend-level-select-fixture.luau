-- Mod frontend hub Luau example: catalog-driven level select and game-flow
-- calls. The repo's generated `sdk/types/postretro.d.luau` surface is guarded
-- by drift tests; this file gives reviewers a committed Luau call-site shape
-- under the dev script fixtures.

local Postretro = require("postretro")
local UI = require("postretro/ui")

local mapCatalog = Postretro.defineMapCatalog({
  {
    id = "campaign-test",
    path = "maps/campaign-test.prl",
    name = "Campaign Test",
    tags = { "campaign", "recommended" },
  },
  {
    id = "combat-demo",
    path = "maps/combat-demo.prl",
    name = "Combat Demo",
    tags = { "campaign", "combat" },
  },
  {
    id = "arena-lights",
    path = "maps/arena-lights.prl",
    name = "Arena Lights",
    tags = { "lighting", "sandbox" },
  },
})

local function hasTag(entry: ModMapEntry, tag: string): boolean
  if entry.tags == nil then
    return false
  end
  for _, value in entry.tags do
    if value == tag then
      return true
    end
  end
  return false
end

local function filteredMaps(tag: string): { ModMapEntry }
  local out = {}
  for _, entry in mapCatalog do
    if hasTag(entry, tag) then
      table.insert(out, entry)
    end
  end
  return out
end

local campaignMaps = filteredMaps("campaign")
local lightingMaps = filteredMaps("lighting")

local function appendLevelButtons(children: { WidgetDescriptor }, entries: { ModMapEntry })
  for _, entry in entries do
    local startMap = Postretro.defineReaction("frontend.start." .. entry.id, UI.loadLevel(entry.id))
    table.insert(children, UI.Button({
      id = "start-" .. entry.id,
      label = entry.name,
      onPress = startMap,
    }))
  end
end

local restartCurrentLevel = Postretro.defineReaction("frontend.restartCurrentLevel", UI.restartLevel())
local backToFrontend = Postretro.defineReaction("frontend.returnToFrontend", UI.returnToFrontend())

local children = {
  UI.Text({ content = "Campaign" }),
}
appendLevelButtons(children, campaignMaps)
table.insert(children, UI.Text({ content = "Lighting" }))
appendLevelButtons(children, lightingMaps)
table.insert(children, UI.Button({
  id = "restart-current-level",
  label = "Restart Current Level",
  onPress = restartCurrentLevel,
}))
table.insert(children, UI.Button({
  id = "return-to-frontend",
  label = "Return To Menu",
  onPress = backToFrontend,
}))
table.insert(children, UI.Button({
  id = "quit-to-menu-action",
  label = "Quit To Menu Action",
  onPress = UI.QUIT_TO_MENU_ACTION,
}))

local levelSelect = UI.defineUiTree({
  name = "frontend.levelSelect.fixture",
  tree = UI.Tree({
    anchor = "center",
    offset = { 0, 0 },
    captureMode = "capture",
    initialFocus = "start-" .. campaignMaps[1].id,
    accessibleName = "Level select",
    role = "group",
  }, UI.VStack({
    gap = 10,
    padding = 18,
    align = "stretch",
    focus = { policy = "linear", wrap = true },
  }, children)),
})

local catalogStartReactions = {}
for _, entry in mapCatalog do
  table.insert(catalogStartReactions, Postretro.defineReaction("frontend.catalog." .. entry.id, UI.loadLevel(entry.id)))
end
table.insert(catalogStartReactions, restartCurrentLevel)
table.insert(catalogStartReactions, backToFrontend)

return Postretro.defineMod({
  name = "frontend-level-select-fixture",
  maps = mapCatalog,
  frontend = {
    menuTree = levelSelect.name,
    backgroundLevel = "arena-lights",
    camera = {
      position = { 0, 1.7, -4 },
      yaw = 0,
      pitch = 0,
    },
  },
  uiTrees = { levelSelect },
  reactions = catalogStartReactions,
})
