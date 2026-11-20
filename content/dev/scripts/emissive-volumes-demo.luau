-- Luau twin of emissive-volumes-demo.ts: the same reactions, identical wire.
-- Values are brightness in [0, 1] across each volume's authored range; a
-- brightness write clears a running animation, a colour write leaves it.
-- See: context/lib/scripting.md §10.2a

local Postretro = require("postretro")

function setupLevel(_ctx)
  local reactions = {}

  local signs = Postretro.getMapEntities("emissive", { tag = "bar_sign" })
  for _, sign in signs do
    -- sign.component: { min, max, brightness, color }
    table.insert(reactions, Postretro.defineReaction("levelLoad", { sequence = sign:flicker({ min = 0.3, max = 1, rate = 6 }) }))
  end
  table.insert(reactions, Postretro.defineReaction("powerCut", { primitive = "setEmissiveBrightness", tag = "bar_sign", args = { brightness = 0 } }))
  table.insert(reactions, Postretro.defineReaction("doorLocked", { primitive = "setEmissiveColor", tag = "status_lamp", args = { color = { 1, 0.05, 0.02 } } }))
  -- The lamp's authored `emissive_color` `40 255 60`, channel / 255.
  table.insert(reactions, Postretro.defineReaction("doorUnlocked", { primitive = "setEmissiveColor", tag = "status_lamp", args = { color = { 40 / 255, 1, 60 / 255 } } }))

  for _, lamp in Postretro.getMapEntities("emissive", { tag = "status_lamp" }) do
    table.insert(reactions, Postretro.defineReaction("levelLoad", { sequence = lamp:pulse({ min = 0.6, max = 1, periodMs = 1500 }) }))
  end
  for _, exit in Postretro.getMapEntities("emissive", { tag = "exit_sign" }) do
    table.insert(reactions, Postretro.defineReaction("levelLoad", { sequence = exit:fade({ from = 0, to = 1, periodMs = 1500 }) }))
  end
  for _, panel in Postretro.getMapEntities("emissive", { tag = "breathe_panel" }) do
    table.insert(reactions, Postretro.defineReaction("levelLoad", { sequence = panel:pulse({ min = 0, max = 1, periodMs = 3000 }) }))
  end

  return { reactions = reactions }
end
