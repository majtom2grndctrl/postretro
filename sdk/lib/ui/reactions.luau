--!strict
-- UI reaction vocabulary: pure builders for the HUD-dynamics reaction surface
-- (M13 Goal E). `onStateCrossing` constructs a state-crossing watcher returned
-- through `setupLevel`'s manifest in the `crossings` field. It never calls back
-- into Rust -- the FFI boundary is the `return` statement of the script entry
-- point.
-- See: context/lib/scripting.md §10.4

--- Crossing condition: fires when the watched slot crosses the threshold in one
--- direction. Exactly one of `below`/`above` is given. `max` is the denominator
--- the threshold is a fraction of; omit it for a raw-value comparison (`max`
--- defaults to `1.0`).
export type CrossingCondition = {
  below: number?,
  above: number?,
  max: number?,
}

--- A state-crossing watcher entry as it appears in `setupLevel`'s manifest
--- `crossings` array. The condition fields are flattened in beside `slot` and
--- `fire`; `fire` is the list of named reactions dispatched (through the shared
--- named-reaction vocabulary) when the crossing occurs.
export type CrossingDescriptor = {
  slot: string,
  below: number?,
  above: number?,
  max: number?,
  fire: {string},
}

local UiReactionsSdk = {}

--- Build a state-crossing watcher. Pure: returns a plain table, does not
--- register anything in the engine. The engine consumes the table when
--- `setupLevel` returns its `crossings` array. On a crossing in the condition's
--- direction the engine fires every reaction in `fire` exactly once, re-arming
--- only after a crossing back. A registration against a non-Number slot warns
--- and is skipped at load.
function UiReactionsSdk.onStateCrossing(
  slot: string,
  condition: CrossingCondition,
  fire: {string}
): CrossingDescriptor
  return {
    slot = slot,
    below = condition.below,
    above = condition.above,
    max = condition.max,
    fire = fire,
  }
end

return UiReactionsSdk
