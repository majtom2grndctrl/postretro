// Private fluent-expression implementation shared by descriptor builders.
// Only public ref types are re-exported by the root SDK.

import type { RuntimeValue } from "postretro";

declare const numBrand: unique symbol;
declare const boolBrand: unique symbol;

/** A numeric literal, fluent expression, or raw `runtime.*` node. */
export type NumberValue = number | NumberRef | RuntimeValue;
/** A boolean literal, fluent expression, or raw `runtime.*` node. */
export type BoolValue = boolean | BoolRef | RuntimeValue;

export interface NumberRef {
  readonly [numBrand]: true;
  plus(n: NumberValue): NumberRef;
  minus(n: NumberValue): NumberRef;
  times(n: NumberValue): NumberRef;
  dividedBy(n: NumberValue): NumberRef;
  clamp(lo: NumberValue, hi: NumberValue): NumberRef;
  lerp(to: NumberValue, t: NumberValue): NumberRef;
  lt(n: NumberValue): BoolRef;
  le(n: NumberValue): BoolRef;
  gt(n: NumberValue): BoolRef;
  ge(n: NumberValue): BoolRef;
  eq(n: NumberValue): BoolRef;
  ne(n: NumberValue): BoolRef;
}

export interface BoolRef {
  readonly [boolBrand]: true;
  and(other: BoolValue): BoolRef;
  or(other: BoolValue): BoolRef;
  not(): BoolRef;
  select(whenTrue: NumberValue, whenFalse: NumberValue): NumberRef;
}

/**
 * Lift raw `runtime.*` output into the fluent impact-expression algebra.
 * `number` and `bool` select the expected result kind; Rust remains the
 * authority that validates the resulting IR at bind time.
 */
export type RuntimeExpressionRefs = Readonly<{
  number(value: RuntimeValue): NumberRef;
  bool(value: RuntimeValue): BoolRef;
}>;

const numberNodes = new WeakMap<object, RuntimeValue>();
const boolNodes = new WeakMap<object, RuntimeValue>();

function constant(value: number | boolean): RuntimeValue {
  return { op: "const", value };
}

export function numberNode(value: NumberValue): RuntimeValue {
  if (typeof value === "number") return constant(value);
  return numberNodes.get(value) ?? (value as RuntimeValue);
}

export function boolNode(value: BoolValue): RuntimeValue {
  if (typeof value === "boolean") return constant(value);
  return boolNodes.get(value) ?? (value as RuntimeValue);
}

export function numberRef(node: RuntimeValue): NumberRef {
  const ref: NumberRef = {
    plus: (n) => numberRef({ op: "add", a: node, b: numberNode(n) }),
    minus: (n) => numberRef({ op: "sub", a: node, b: numberNode(n) }),
    times: (n) => numberRef({ op: "mul", a: node, b: numberNode(n) }),
    dividedBy: (n) => numberRef({ op: "div", a: node, b: numberNode(n) }),
    clamp: (lo, hi) => numberRef({ op: "clamp", x: node, lo: numberNode(lo), hi: numberNode(hi) }),
    lerp: (to, t) => numberRef({ op: "lerp", a: node, b: numberNode(to), t: numberNode(t) }),
    lt: (n) => boolRef({ op: "lt", a: node, b: numberNode(n) }),
    le: (n) => boolRef({ op: "le", a: node, b: numberNode(n) }),
    gt: (n) => boolRef({ op: "gt", a: node, b: numberNode(n) }),
    ge: (n) => boolRef({ op: "ge", a: node, b: numberNode(n) }),
    eq: (n) => boolRef({ op: "eq", a: node, b: numberNode(n) }),
    ne: (n) => boolRef({ op: "ne", a: node, b: numberNode(n) }),
  } as NumberRef;
  numberNodes.set(ref, node);
  return Object.freeze(ref);
}

export function boolRef(node: RuntimeValue): BoolRef {
  const ref: BoolRef = {
    and: (other) => boolRef({ op: "select", cond: node, a: boolNode(other), b: constant(false) }),
    or: (other) => boolRef({ op: "select", cond: node, a: constant(true), b: boolNode(other) }),
    not: () => boolRef({ op: "select", cond: node, a: constant(false), b: constant(true) }),
    select: (whenTrue, whenFalse) => numberRef({
      op: "select",
      cond: node,
      a: numberNode(whenTrue),
      b: numberNode(whenFalse),
    }),
  } as BoolRef;
  boolNodes.set(ref, node);
  return Object.freeze(ref);
}

