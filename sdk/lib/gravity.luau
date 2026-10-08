--!strict
-- World gravity: thin wrappers over the gravity primitives.
-- See: context/lib/scripting.md §7 (SDK Type Definitions)

local GravitySdk = {}

--- Current world gravity in m/s² (negative = downward; positive = upward).
--- Seeded from worldspawn `initialGravity` at level load.
function GravitySdk.getGravity(): number
  return worldGetGravity()
end

--- Set world gravity in m/s². Non-finite values are ignored with a warning.
function GravitySdk.setGravity(value: number): ()
  worldSetGravity(value)
end

return GravitySdk
