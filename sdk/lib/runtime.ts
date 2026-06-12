// Behavior IR vocabulary: pure constructors for the typed command buffer.
// Each builder assembles an `IrNode` object as plain data and returns it —
// no FFI side effect. Nodes cross the boundary through the normal `setupMod`
// / `setupLevel` return path, never as a primitive call. The constructors are
// namespaced under a single `ir` global (mirroring `world`) so generic op
// names like `add` / `eq` / `select` do not collide with author symbols.
// See: context/lib/scripting.md §11 (Typed Command Buffer), §12.

import type {
  IrNode,
  IrConst,
  IrInput,
  IrAdd,
  IrSub,
  IrMul,
  IrDiv,
  IrClamp,
  IrLerp,
  IrLt,
  IrLe,
  IrGt,
  IrGe,
  IrEq,
  IrNe,
  IrSelect,
} from "postretro";

/** Pure builder vocabulary for the behavior IR. See the `Ir` interface in
 * `postretro.d.ts` for the per-builder contracts. */
export const ir = {
  /** Literal scalar leaf. `const` is reserved, so the builder is `constant`. */
  constant(value: number | boolean): IrConst {
    return { op: "const", value };
  },
  /** Named-input leaf, bound to live state by name in the Rust evaluator. */
  input(name: string): IrInput {
    return { op: "input", name };
  },
  /** `a + b` (number). */
  add(a: IrNode, b: IrNode): IrAdd {
    return { op: "add", a, b };
  },
  /** `a - b` (number). */
  sub(a: IrNode, b: IrNode): IrSub {
    return { op: "sub", a, b };
  },
  /** `a * b` (number). */
  mul(a: IrNode, b: IrNode): IrMul {
    return { op: "mul", a, b };
  },
  /** `a / b` (number). */
  div(a: IrNode, b: IrNode): IrDiv {
    return { op: "div", a, b };
  },
  /** Clamp `x` to `[lo, hi]` (number). */
  clamp(x: IrNode, lo: IrNode, hi: IrNode): IrClamp {
    return { op: "clamp", x, lo, hi };
  },
  /** Linear interpolation between `a` and `b` by `t` (number). */
  lerp(a: IrNode, b: IrNode, t: IrNode): IrLerp {
    return { op: "lerp", a, b, t };
  },
  /** `a < b` (boolean). */
  lt(a: IrNode, b: IrNode): IrLt {
    return { op: "lt", a, b };
  },
  /** `a <= b` (boolean). */
  le(a: IrNode, b: IrNode): IrLe {
    return { op: "le", a, b };
  },
  /** `a > b` (boolean). */
  gt(a: IrNode, b: IrNode): IrGt {
    return { op: "gt", a, b };
  },
  /** `a >= b` (boolean). */
  ge(a: IrNode, b: IrNode): IrGe {
    return { op: "ge", a, b };
  },
  /** `a == b` (boolean). */
  eq(a: IrNode, b: IrNode): IrEq {
    return { op: "eq", a, b };
  },
  /** `a != b` (boolean). */
  ne(a: IrNode, b: IrNode): IrNe {
    return { op: "ne", a, b };
  },
  /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
  select(cond: IrNode, a: IrNode, b: IrNode): IrSelect {
    return { op: "select", cond, a, b };
  },
};
