--!strict
-- Behavior IR vocabulary: pure constructors for the typed command buffer.
-- Each builder assembles an `IrNode` table as plain data and returns it -- no
-- FFI side effect. Nodes cross the boundary through the normal `setupMod` /
-- `setupLevel` return path, never as a primitive call. The constructors are
-- namespaced under a single `ir` global (mirroring `world`) so generic op
-- names like `add` / `eq` / `select` do not collide with author symbols.
-- See: context/lib/scripting.md §11 (Typed Command Buffer), §12.
--
-- `IrNode` and the per-opcode variants (`IrConst`, `IrAdd`, ...) are declared
-- ambiently in the generated `sdk/types/postretro.d.luau` definitions file.

local ir = {
  --- Literal scalar leaf. `const` is a reserved word, so the builder is
  --- `constant`. `value` is a number or boolean.
  constant = function(value: number | boolean): IrConst
    return { op = "const", value = value }
  end,
  --- Named-input leaf, bound to live state by name in the Rust evaluator.
  input = function(name: string): IrInput
    return { op = "input", name = name }
  end,
  --- `a + b` (number).
  add = function(a: IrNode, b: IrNode): IrAdd
    return { op = "add", a = a, b = b }
  end,
  --- `a - b` (number).
  sub = function(a: IrNode, b: IrNode): IrSub
    return { op = "sub", a = a, b = b }
  end,
  --- `a * b` (number).
  mul = function(a: IrNode, b: IrNode): IrMul
    return { op = "mul", a = a, b = b }
  end,
  --- `a / b` (number).
  div = function(a: IrNode, b: IrNode): IrDiv
    return { op = "div", a = a, b = b }
  end,
  --- Clamp `x` to `[lo, hi]` (number).
  clamp = function(x: IrNode, lo: IrNode, hi: IrNode): IrClamp
    return { op = "clamp", x = x, lo = lo, hi = hi }
  end,
  --- Linear interpolation between `a` and `b` by `t` (number).
  lerp = function(a: IrNode, b: IrNode, t: IrNode): IrLerp
    return { op = "lerp", a = a, b = b, t = t }
  end,
  --- `a < b` (boolean).
  lt = function(a: IrNode, b: IrNode): IrLt
    return { op = "lt", a = a, b = b }
  end,
  --- `a <= b` (boolean).
  le = function(a: IrNode, b: IrNode): IrLe
    return { op = "le", a = a, b = b }
  end,
  --- `a > b` (boolean).
  gt = function(a: IrNode, b: IrNode): IrGt
    return { op = "gt", a = a, b = b }
  end,
  --- `a >= b` (boolean).
  ge = function(a: IrNode, b: IrNode): IrGe
    return { op = "ge", a = a, b = b }
  end,
  --- `a == b` (boolean).
  eq = function(a: IrNode, b: IrNode): IrEq
    return { op = "eq", a = a, b = b }
  end,
  --- `a ~= b` (boolean).
  ne = function(a: IrNode, b: IrNode): IrNe
    return { op = "ne", a = a, b = b }
  end,
  --- Branchless select: `cond ? a : b`. `a` and `b` share a type.
  select = function(cond: IrNode, a: IrNode, b: IrNode): IrSelect
    return { op = "select", cond = cond, a = a, b = b }
  end,
}

return ir
