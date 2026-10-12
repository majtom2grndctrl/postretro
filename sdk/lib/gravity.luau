--!strict
-- World gravity: thin wrappers over the gravity primitives.
-- See: context/lib/scripting.md §7 (SDK Type Definitions)

local GravitySdk = {}

--- Level default gravity in m/s² (negative = downward; positive = upward):
--- the gravity outside every gravity volume. Seeded from worldspawn
--- `initialGravity` at level load. Inside a `gravity_volume` the volume's own
--- value holds; this still returns the level default.
function GravitySdk.getGravity(): number
  return worldGetGravity()
end

--- Set the level default gravity in m/s²: the gravity outside every gravity
--- volume; a `gravity_volume`'s value holds inside it regardless. Non-finite
--- values are ignored with a warning.
function GravitySdk.setGravity(value: number): ()
  worldSetGravity(value)
end

return GravitySdk
