--!strict
-- Generic keyframe authoring helpers (`Keyframe`, `timeline`, `sequence`).
-- See: context/lib/scripting.md §7

--- Per-channel keyframe format accepted by `timeline` and `sequence`.
--- First slot is a timestamp (absolute for `timeline`, delta for
--- `sequence`); remaining slots carry the value for that keyframe.
export type Keyframe = {number}

local Keyframes = {}

local function validateKeyframes(keyframes: {Keyframe}, isSequence: boolean)
  local label = if isSequence then "sequence" else "timeline"
  if type(keyframes) ~= "table" or #keyframes == 0 then
    error(`{label}: keyframes must be a non-empty array`)
  end
  local first = keyframes[1]
  if type(first) ~= "table" or #first == 0 then
    error(`{label}: entry 0 is empty`)
  end
  local arity = #first

  local prevT: number = -math.huge
  for i, kf in ipairs(keyframes) do
    -- Entry indices reported in the error messages are 0-based to match
    -- the TypeScript vocabulary (so tests comparing messages line up).
    local idx = i - 1
    if type(kf) ~= "table" then
      error(`{label}: entry {idx} is not an array`)
    end
    if #kf ~= arity then
      error(`{label}: entry {idx} has arity {#kf}, expected {arity}`)
    end
    for s = 1, #kf do
      local v = kf[s]
      -- Luau: non-finite is NaN or +/-inf. `v ~= v` catches NaN.
      if type(v) ~= "number" or v ~= v or v == math.huge or v == -math.huge then
        error(`{label}: entry {idx} slot {s - 1} is not a finite number`)
      end
    end
    local t = kf[1]
    if i > 1 and t <= prevT then
      error(`{label}: entry {idx} timestamp {t} is not strictly greater than previous {prevT}`)
    end
    prevT = t
  end
end

--- Validates a list of `{absolute_ms, ...value}` keyframes and returns
--- it unchanged. Raises a descriptive `error` naming the offending entry
--- if:
---
--- - The list is empty.
--- - Any entry is empty or has a different arity from the first.
--- - Any slot is a non-finite number.
--- - Timestamps are not strictly increasing.
---
--- The engine consumes `{absolute_ms, ...value}` directly; `timeline`
--- exists purely for shape validation so authoring mistakes surface
--- instead of being silently dropped.
function Keyframes.timeline(keyframes: {Keyframe}): {Keyframe}
  validateKeyframes(keyframes, false)
  return keyframes
end

--- Accepts `{delta_ms, ...value}` keyframes and returns the canonical
--- `{absolute_ms, ...value}` form by accumulating deltas. The first
--- entry is passed through verbatim; subsequent timestamps are the
--- running sum of all preceding deltas plus the current delta.
---
--- Validates the accumulated timeline with the same rules as `timeline`,
--- so non-positive deltas (which would produce non-monotonic absolute
--- timestamps after the first keyframe) raise a descriptive error.
function Keyframes.sequence(keyframes: {Keyframe}): {Keyframe}
  if type(keyframes) ~= "table" or #keyframes == 0 then
    error("sequence: keyframes must be a non-empty array")
  end
  local first = keyframes[1]
  if type(first) ~= "table" or #first == 0 then
    error("sequence: entry 0 is empty")
  end
  local arity = #first

  local out: {Keyframe} = {}
  -- Copy the first entry so we don't alias the caller's input table.
  local firstCopy: Keyframe = {}
  for s = 1, arity do
    firstCopy[s] = first[s]
  end
  out[1] = firstCopy

  for i = 2, #keyframes do
    local kf = keyframes[i]
    local idx = i - 1 -- 0-based for error parity with TypeScript.
    if type(kf) ~= "table" then
      error(`sequence: entry {idx} is not an array`)
    end
    if #kf ~= arity then
      error(`sequence: entry {idx} has arity {#kf}, expected {arity}`)
    end
    for s = 1, #kf do
      local v = kf[s]
      if type(v) ~= "number" or v ~= v or v == math.huge or v == -math.huge then
        error(`sequence: entry {idx} slot {s - 1} is not a finite number`)
      end
    end
    local delta = kf[1]
    local prevT = out[i - 1][1]
    local absT = prevT + delta
    if absT <= prevT then
      error(`sequence: entry {idx} delta {delta} produces non-monotonic timestamp (prev={prevT}, next={absT})`)
    end
    local copy: Keyframe = {}
    for s = 1, arity do
      copy[s] = kf[s]
    end
    copy[1] = absT
    out[i] = copy
  end

  -- Defensive re-validation of the accumulated output. Catches any
  -- inconsistency we didn't already flag (e.g. the first entry's fields
  -- being non-finite).
  validateKeyframes(out, true)
  return out
end

return Keyframes
