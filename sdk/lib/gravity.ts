// World gravity: thin wrappers over the gravity primitives.
// See: context/lib/scripting.md §10.1

import { worldGetGravity, worldSetGravity } from "postretro";

/**
 * Current world gravity in m/s². Negative = downward (Earth = -9.81),
 * positive = upward. Seeded from the worldspawn `initialGravity` KVP at
 * level load and persists until the next level load or `setGravity` call.
 */
export function getGravity(): number {
  return worldGetGravity();
}

/**
 * Set the world gravity in m/s². Negative = downward, positive = upward.
 * NaN and non-finite values are ignored with a warning. Effect is immediate
 * and persists until the next level load or another `setGravity` call.
 */
export function setGravity(value: number): void {
  worldSetGravity(value);
}
