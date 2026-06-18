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
type Operand = RuntimeValue | number | boolean

-- Wrap a bare number/boolean literal into a `const` node; pass an existing node
-- through unchanged. The wrapping rule is identical in `runtime.ts` so the two
-- runtimes canonicalize to byte-identical IR.
local function wrap(value: Operand): RuntimeValue
  if type(value) == "number" or type(value) == "boolean" then
    return { op = "const", value = value }
  end
  return value
end

local runtime = {
  --- Literal scalar leaf. `const` is a reserved word, so the builder is
  --- `constant`. `value` is a number or boolean.
  constant = function(value: number | boolean): RuntimeConst
    return { op = "const", value = value }
  end,
  --- Named-input leaf, bound to live state by name in the Rust evaluator.
  read = function(name: string): RuntimeRead
    return { op = "input", name = name }
  end,
  --- `a + b` (number).
  add = function(a: Operand, b: Operand): RuntimeAdd
    return { op = "add", a = wrap(a), b = wrap(b) }
  end,
  --- `a - b` (number).
  sub = function(a: Operand, b: Operand): RuntimeSub
    return { op = "sub", a = wrap(a), b = wrap(b) }
  end,
  --- `a * b` (number).
  mul = function(a: Operand, b: Operand): RuntimeMul
    return { op = "mul", a = wrap(a), b = wrap(b) }
  end,
  --- `a / b` (number).
  div = function(a: Operand, b: Operand): RuntimeDiv
    return { op = "div", a = wrap(a), b = wrap(b) }
  end,
  --- Clamp `x` to `[lo, hi]` (number).
  clamp = function(x: Operand, lo: Operand, hi: Operand): RuntimeClamp
    return { op = "clamp", x = wrap(x), lo = wrap(lo), hi = wrap(hi) }
  end,
  --- Linear interpolation between `a` and `b` by `t` (number).
  lerp = function(a: Operand, b: Operand, t: Operand): RuntimeLerp
    return { op = "lerp", a = wrap(a), b = wrap(b), t = wrap(t) }
  end,
  --- `a < b` (boolean).
  lt = function(a: Operand, b: Operand): RuntimeLt
    return { op = "lt", a = wrap(a), b = wrap(b) }
  end,
  --- `a <= b` (boolean).
  le = function(a: Operand, b: Operand): RuntimeLe
    return { op = "le", a = wrap(a), b = wrap(b) }
  end,
  --- `a > b` (boolean).
  gt = function(a: Operand, b: Operand): RuntimeGt
    return { op = "gt", a = wrap(a), b = wrap(b) }
  end,
  --- `a >= b` (boolean).
  ge = function(a: Operand, b: Operand): RuntimeGe
    return { op = "ge", a = wrap(a), b = wrap(b) }
  end,
  --- `a == b` (boolean).
  eq = function(a: Operand, b: Operand): RuntimeEq
    return { op = "eq", a = wrap(a), b = wrap(b) }
  end,
  --- `a ~= b` (boolean).
  ne = function(a: Operand, b: Operand): RuntimeNe
    return { op = "ne", a = wrap(a), b = wrap(b) }
  end,
  --- Branchless select: `cond ? a : b`. `a` and `b` share a type.
  select = function(cond: Operand, a: Operand, b: Operand): RuntimeSelect
    return { op = "select", cond = wrap(cond), a = wrap(a), b = wrap(b) }
  end,
}

return runtime
