--!strict
-- Runtime-value vocabulary: pure constructors for the typed command buffer.
-- Each builder assembles a `RuntimeValue` table as plain data and returns it --
-- no FFI side effect. Nodes cross the boundary through manifest data
-- (`ModManifest` / `setupLevel`), never as a primitive call. The constructors are
-- namespaced under a single `runtime` global (mirroring `world`) so generic op
-- names like `add` / `eq` / `select` do not collide with author symbols.
-- See: context/lib/scripting.md §11 (Typed Command Buffer), §12.
--
-- `RuntimeValue` and the per-opcode variants (`RuntimeConst`, `RuntimeAdd`, ...)
-- are declared ambiently in the generated `sdk/types/postretro.d.luau`
-- definitions file.

-- A builder argument: either an already-built node or a bare literal that is
-- auto-wrapped into a `const` node.
type Operand = RuntimeValue | StateRef<any> | number | boolean

local function input(name: string | StateRef<any>): RuntimeRead
  if type(name) == "string" then
    return { op = "input", name = name }
  end
  local node = { op = "input", name = (name :: any).slot }
  if (name :: any).owner ~= nil then
    node.owner = (name :: any).owner
  end
  return node
end

-- Wrap a bare number/boolean literal into a `const` node; pass an existing node
-- through unchanged. The wrapping rule is identical in `runtime.ts` so the two
-- runtimes canonicalize to byte-identical IR.
local function wrap(value: Operand): RuntimeValue
  if type(value) == "number" or type(value) == "boolean" then
    return { op = "const", value = value }
  end
  if type(value) == "table" and type((value :: any).slot) == "string" then
    return input(value :: StateRef<any>)
  end
  return value
end

-- Fluent methods live in a shared metatable rather than on the node table, so
-- they are never serialized into descriptor data. `brain.luau` creates its
-- pre-wrapped leaves through `runtime.read`, which applies this metatable
-- before those leaves are frozen.
local guard_methods = {}
local guard_metatable = { __index = guard_methods }

local function node(value: any): RuntimeValue
  return setmetatable(value, guard_metatable)
end

guard_methods.le = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "le", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.ge = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "ge", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.lt = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "lt", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.gt = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "gt", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.eq = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "eq", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.ne = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "ne", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods.between = function(self: RuntimeValue, lo: Operand, hi: Operand): RuntimeGuardNode
  return node({
    op = "and",
    a = node({ op = "ge", a = self, b = wrap(lo) }),
    b = node({ op = "le", a = self, b = wrap(hi) }),
  }) :: RuntimeGuardNode
end
guard_methods["and"] = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "and", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods["or"] = function(self: RuntimeValue, other: Operand): RuntimeGuardNode
  return node({ op = "or", a = self, b = wrap(other) }) :: RuntimeGuardNode
end
guard_methods["not"] = function(self: RuntimeValue): RuntimeGuardNode
  return node({ op = "not", x = self }) :: RuntimeGuardNode
end

local runtime = {
  --- Literal scalar leaf. `const` is a reserved word, so the builder is
  --- `constant`. `value` is a number or boolean.
  constant = function(value: number | boolean): RuntimeConst
    return node({ op = "const", value = value }) :: RuntimeConst
  end,
  --- Named-input leaf, bound to live state by name in the Rust evaluator.
  read = function(name: string | StateRef<any>): RuntimeRead
    return node(input(name)) :: RuntimeRead
  end,
  --- `a + b` (number).
  add = function(a: Operand, b: Operand): RuntimeAdd
    return node({ op = "add", a = wrap(a), b = wrap(b) }) :: RuntimeAdd
  end,
  --- `a - b` (number).
  sub = function(a: Operand, b: Operand): RuntimeSub
    return node({ op = "sub", a = wrap(a), b = wrap(b) }) :: RuntimeSub
  end,
  --- `a * b` (number).
  mul = function(a: Operand, b: Operand): RuntimeMul
    return node({ op = "mul", a = wrap(a), b = wrap(b) }) :: RuntimeMul
  end,
  --- `a / b` (number).
  div = function(a: Operand, b: Operand): RuntimeDiv
    return node({ op = "div", a = wrap(a), b = wrap(b) }) :: RuntimeDiv
  end,
  --- Clamp `x` to `[lo, hi]` (number).
  clamp = function(x: Operand, lo: Operand, hi: Operand): RuntimeClamp
    return node({ op = "clamp", x = wrap(x), lo = wrap(lo), hi = wrap(hi) }) :: RuntimeClamp
  end,
  --- Linear interpolation between `a` and `b` by `t` (number).
  lerp = function(a: Operand, b: Operand, t: Operand): RuntimeLerp
    return node({ op = "lerp", a = wrap(a), b = wrap(b), t = wrap(t) }) :: RuntimeLerp
  end,
  --- `a < b` (boolean).
  lt = function(a: Operand, b: Operand): RuntimeLt
    return node({ op = "lt", a = wrap(a), b = wrap(b) }) :: RuntimeLt
  end,
  --- `a <= b` (boolean).
  le = function(a: Operand, b: Operand): RuntimeLe
    return node({ op = "le", a = wrap(a), b = wrap(b) }) :: RuntimeLe
  end,
  --- `a > b` (boolean).
  gt = function(a: Operand, b: Operand): RuntimeGt
    return node({ op = "gt", a = wrap(a), b = wrap(b) }) :: RuntimeGt
  end,
  --- `a >= b` (boolean).
  ge = function(a: Operand, b: Operand): RuntimeGe
    return node({ op = "ge", a = wrap(a), b = wrap(b) }) :: RuntimeGe
  end,
  --- `a == b` (boolean).
  eq = function(a: Operand, b: Operand): RuntimeEq
    return node({ op = "eq", a = wrap(a), b = wrap(b) }) :: RuntimeEq
  end,
  --- `a ~= b` (boolean).
  ne = function(a: Operand, b: Operand): RuntimeNe
    return node({ op = "ne", a = wrap(a), b = wrap(b) }) :: RuntimeNe
  end,
  --- Boolean conjunction. Prefer this over native `and` on node tables.
  ["and"] = function(a: Operand, b: Operand): RuntimeAnd
    return node({ op = "and", a = wrap(a), b = wrap(b) }) :: RuntimeAnd
  end,
  --- Boolean disjunction. Prefer this over native `or` on node tables.
  ["or"] = function(a: Operand, b: Operand): RuntimeOr
    return node({ op = "or", a = wrap(a), b = wrap(b) }) :: RuntimeOr
  end,
  --- Boolean inversion. Prefer this over native `not` on a node table.
  ["not"] = function(x: Operand): RuntimeNot
    return node({ op = "not", x = wrap(x) }) :: RuntimeNot
  end,
  --- Branchless select: `cond ? a : b`. `a` and `b` share a type.
  select = function(cond: Operand, a: Operand, b: Operand): RuntimeSelect
    return node({ op = "select", cond = wrap(cond), a = wrap(a), b = wrap(b) }) :: RuntimeSelect
  end,
}

return runtime
