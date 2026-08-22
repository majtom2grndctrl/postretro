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
  RuntimeAnd,
  RuntimeOr,
  RuntimeNot,
  RuntimeSelect,
  RuntimeGuardNode,
} from "postretro";
import type { StateRef } from "./data_script";

/** A builder argument: either an already-built node or a bare literal that is
 * auto-wrapped into a `const` node. */
type Operand = RuntimeValue | StateRef<unknown> | number | boolean;

/** The fluent guard methods live on a prototype rather than the node itself, so
 * they never enter the descriptor wire. The static typedef exposes the same
 * methods as `RuntimeGuardNode`; raw object literals remain ordinary
 * `RuntimeValue`s. */
interface GuardMethods {
  le(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  ge(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  lt(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  gt(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  eq(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  ne(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  between(this: RuntimeValue, lo: Operand, hi: Operand): RuntimeGuardNode;
  and(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  or(this: RuntimeValue, other: Operand): RuntimeGuardNode;
  not(this: RuntimeValue): RuntimeGuardNode;
}

function input(name: string | StateRef<unknown>): RuntimeRead {
  if (typeof name === "string") return { op: "input", name };
  const owner = "owner" in name ? name.owner : undefined;
  return owner === undefined
    ? { op: "input", name: name.slot }
    : { op: "input", name: name.slot, owner };
}

/** Wrap a bare `number`/`boolean` literal into a `const` node; pass an existing
 * node through unchanged. The wrapping rule is identical in `runtime.luau` so
 * the two runtimes canonicalize to byte-identical IR. */
function wrap(value: Operand): RuntimeValue {
  if (typeof value === "number" || typeof value === "boolean") {
    return { op: "const", value };
  }
  if (value !== null && typeof value === "object" && "slot" in value && typeof value.slot === "string") {
    return input(value as StateRef<unknown>);
  }
  return value;
}

/** Attach the fluent methods as inherited, non-wire behavior. Every runtime
 * builder passes through here before its node can become a frozen brain leaf. */
function node<T extends RuntimeValue>(value: T): T {
  Object.setPrototypeOf(value, GUARD_METHODS);
  return value;
}

const GUARD_METHODS: GuardMethods = Object.freeze({
  le(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "le", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  ge(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "ge", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  lt(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "lt", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  gt(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "gt", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  eq(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "eq", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  ne(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "ne", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  between(this: RuntimeValue, lo: Operand, hi: Operand): RuntimeGuardNode {
    return node({
      op: "and",
      a: node({ op: "ge", a: this, b: wrap(lo) }),
      b: node({ op: "le", a: this, b: wrap(hi) }),
    }) as RuntimeGuardNode;
  },
  and(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "and", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  or(this: RuntimeValue, other: Operand): RuntimeGuardNode {
    return node({ op: "or", a: this, b: wrap(other) }) as RuntimeGuardNode;
  },
  not(this: RuntimeValue): RuntimeGuardNode {
    return node({ op: "not", x: this }) as RuntimeGuardNode;
  },
});

/** Pure builder vocabulary for runtime values. See the `Runtime` interface in
 * `postretro.d.ts` for the per-builder contracts. */
export const runtime = {
  /** Literal scalar leaf. `const` is reserved, so the builder is `constant`. */
  constant(value: number | boolean): RuntimeConst {
    return node({ op: "const", value }) as RuntimeConst;
  },
  /** Named-input leaf, bound to live state by name in the Rust evaluator. */
  read(name: string | StateRef<unknown>): RuntimeRead {
    return node(input(name)) as RuntimeRead;
  },
  /** `a + b` (number). */
  add(a: Operand, b: Operand): RuntimeAdd {
    return node({ op: "add", a: wrap(a), b: wrap(b) }) as RuntimeAdd;
  },
  /** `a - b` (number). */
  sub(a: Operand, b: Operand): RuntimeSub {
    return node({ op: "sub", a: wrap(a), b: wrap(b) }) as RuntimeSub;
  },
  /** `a * b` (number). */
  mul(a: Operand, b: Operand): RuntimeMul {
    return node({ op: "mul", a: wrap(a), b: wrap(b) }) as RuntimeMul;
  },
  /** `a / b` (number). */
  div(a: Operand, b: Operand): RuntimeDiv {
    return node({ op: "div", a: wrap(a), b: wrap(b) }) as RuntimeDiv;
  },
  /** Clamp `x` to `[lo, hi]` (number). */
  clamp(x: Operand, lo: Operand, hi: Operand): RuntimeClamp {
    return node({ op: "clamp", x: wrap(x), lo: wrap(lo), hi: wrap(hi) }) as RuntimeClamp;
  },
  /** Linear interpolation between `a` and `b` by `t` (number). */
  lerp(a: Operand, b: Operand, t: Operand): RuntimeLerp {
    return node({ op: "lerp", a: wrap(a), b: wrap(b), t: wrap(t) }) as RuntimeLerp;
  },
  /** `a < b` (boolean). */
  lt(a: Operand, b: Operand): RuntimeLt {
    return node({ op: "lt", a: wrap(a), b: wrap(b) }) as RuntimeLt;
  },
  /** `a <= b` (boolean). */
  le(a: Operand, b: Operand): RuntimeLe {
    return node({ op: "le", a: wrap(a), b: wrap(b) }) as RuntimeLe;
  },
  /** `a > b` (boolean). */
  gt(a: Operand, b: Operand): RuntimeGt {
    return node({ op: "gt", a: wrap(a), b: wrap(b) }) as RuntimeGt;
  },
  /** `a >= b` (boolean). */
  ge(a: Operand, b: Operand): RuntimeGe {
    return node({ op: "ge", a: wrap(a), b: wrap(b) }) as RuntimeGe;
  },
  /** `a == b` (boolean). */
  eq(a: Operand, b: Operand): RuntimeEq {
    return node({ op: "eq", a: wrap(a), b: wrap(b) }) as RuntimeEq;
  },
  /** `a != b` (boolean). */
  ne(a: Operand, b: Operand): RuntimeNe {
    return node({ op: "ne", a: wrap(a), b: wrap(b) }) as RuntimeNe;
  },
  /** Boolean conjunction: `a && b` in IR, not JavaScript truthiness. */
  and(a: Operand, b: Operand): RuntimeAnd {
    return node({ op: "and", a: wrap(a), b: wrap(b) }) as RuntimeAnd;
  },
  /** Boolean disjunction: `a || b` in IR, not JavaScript truthiness. */
  or(a: Operand, b: Operand): RuntimeOr {
    return node({ op: "or", a: wrap(a), b: wrap(b) }) as RuntimeOr;
  },
  /** Boolean inversion. Prefer this over the native `!` operator on a node. */
  not(x: Operand): RuntimeNot {
    return node({ op: "not", x: wrap(x) }) as RuntimeNot;
  },
  /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
  select(cond: Operand, a: Operand, b: Operand): RuntimeSelect {
    return node({ op: "select", cond: wrap(cond), a: wrap(a), b: wrap(b) }) as RuntimeSelect;
  },
};
