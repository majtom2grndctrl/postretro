// Runtime-value vocabulary: pure constructors for the typed command buffer.
// Each builder assembles a `RuntimeValue` object as plain data and returns it —
// no FFI side effect. Nodes cross the boundary through manifest data
// (`ModManifest` / `setupLevel`), never as a primitive call. The constructors are
// namespaced under a single `runtime` global (mirroring `world`) so generic op
// names like `add` / `eq` / `select` do not collide with author symbols.
// See: context/lib/scripting.md §11 (Typed Command Buffer), §12.

import type {
  RuntimeValue,
  RuntimeConst,
  RuntimeRead,
  RuntimeAdd,
  RuntimeSub,
  RuntimeMul,
  RuntimeDiv,
  RuntimeClamp,
  RuntimeLerp,
  RuntimeLt,
  RuntimeLe,
  RuntimeGt,
  RuntimeGe,
  RuntimeEq,
  RuntimeNe,
  RuntimeSelect,
} from "postretro";

/** A builder argument: either an already-built node or a bare literal that is
 * auto-wrapped into a `const` node. */
type Operand = RuntimeValue | number | boolean;

/** Wrap a bare `number`/`boolean` literal into a `const` node; pass an existing
 * node through unchanged. The wrapping rule is identical in `runtime.luau` so
 * the two runtimes canonicalize to byte-identical IR. */
function wrap(value: Operand): RuntimeValue {
  if (typeof value === "number" || typeof value === "boolean") {
    return { op: "const", value };
  }
  return value;
}

/** Pure builder vocabulary for runtime values. See the `Runtime` interface in
 * `postretro.d.ts` for the per-builder contracts. */
export const runtime = {
  /** Literal scalar leaf. `const` is reserved, so the builder is `constant`. */
  constant(value: number | boolean): RuntimeConst {
    return { op: "const", value };
  },
  /** Named-input leaf, bound to live state by name in the Rust evaluator. */
  read(name: string): RuntimeRead {
    return { op: "input", name };
  },
  /** `a + b` (number). */
  add(a: Operand, b: Operand): RuntimeAdd {
    return { op: "add", a: wrap(a), b: wrap(b) };
  },
  /** `a - b` (number). */
  sub(a: Operand, b: Operand): RuntimeSub {
    return { op: "sub", a: wrap(a), b: wrap(b) };
  },
  /** `a * b` (number). */
  mul(a: Operand, b: Operand): RuntimeMul {
    return { op: "mul", a: wrap(a), b: wrap(b) };
  },
  /** `a / b` (number). */
  div(a: Operand, b: Operand): RuntimeDiv {
    return { op: "div", a: wrap(a), b: wrap(b) };
  },
  /** Clamp `x` to `[lo, hi]` (number). */
  clamp(x: Operand, lo: Operand, hi: Operand): RuntimeClamp {
    return { op: "clamp", x: wrap(x), lo: wrap(lo), hi: wrap(hi) };
  },
  /** Linear interpolation between `a` and `b` by `t` (number). */
  lerp(a: Operand, b: Operand, t: Operand): RuntimeLerp {
    return { op: "lerp", a: wrap(a), b: wrap(b), t: wrap(t) };
  },
  /** `a < b` (boolean). */
  lt(a: Operand, b: Operand): RuntimeLt {
    return { op: "lt", a: wrap(a), b: wrap(b) };
  },
  /** `a <= b` (boolean). */
  le(a: Operand, b: Operand): RuntimeLe {
    return { op: "le", a: wrap(a), b: wrap(b) };
  },
  /** `a > b` (boolean). */
  gt(a: Operand, b: Operand): RuntimeGt {
    return { op: "gt", a: wrap(a), b: wrap(b) };
  },
  /** `a >= b` (boolean). */
  ge(a: Operand, b: Operand): RuntimeGe {
    return { op: "ge", a: wrap(a), b: wrap(b) };
  },
  /** `a == b` (boolean). */
  eq(a: Operand, b: Operand): RuntimeEq {
    return { op: "eq", a: wrap(a), b: wrap(b) };
  },
  /** `a != b` (boolean). */
  ne(a: Operand, b: Operand): RuntimeNe {
    return { op: "ne", a: wrap(a), b: wrap(b) };
  },
  /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
  select(cond: Operand, a: Operand, b: Operand): RuntimeSelect {
    return { op: "select", cond: wrap(cond), a: wrap(a), b: wrap(b) };
  },
};
