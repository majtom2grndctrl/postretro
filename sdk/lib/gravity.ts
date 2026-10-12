// World gravity: thin wrappers over the gravity primitives.
// See: context/lib/scripting.md §7 (SDK Type Definitions)

import { worldGetGravity, worldSetGravity } from "postretro";

/**
 * Level default gravity in m/s²: the gravity outside every gravity volume.
 * Negative = downward (default -20.32), positive = upward. Seeded from the
 * worldspawn `initialGravity` KVP at level load and persists until the next
 * level load or `setGravity` call. Inside a `gravity_volume` the volume's own
 * value holds; this still returns the level default.
 */
export function getGravity(): number {
  return worldGetGravity();
}

/**
 * Set the level default gravity in m/s²: the gravity outside every gravity
 * volume. Negative = downward, positive = upward. A `gravity_volume`'s value
 * holds inside it regardless. NaN and non-finite values are ignored with a
 * warning. Effect is immediate and persists until the next level load or
 * another `setGravity` call.
 */
export function setGravity(value: number): void {
  worldSetGravity(value);
}
