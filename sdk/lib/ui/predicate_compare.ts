// Numeric `compare` validation shared by the UI factories' predicate cloning
// and `stateCompare`. Mirrors `predicate_compare.rs`; the Luau factories carry
// the same checks inline (the Luau prelude has no module loader).

import type { PredicateCompare, PredicateCompareOp } from "./widgets";

/** The ordered relations a numeric predicate tests, in wire spelling. */
export const COMPARE_OPS: readonly PredicateCompareOp[] = ["greaterThan", "lessThan", "atLeast", "atMost"];

/**
 * Validate + clone a predicate's `compare` prop. Errors name the factory and the
 * prop's key path (`VStack: \`visibleWhen.compare.value\` ...`).
 */
export function buildPredicateCompare(value: unknown, field: string, factory: string): PredicateCompare {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${factory}: \`${field}.compare\` must be an object`);
  }
  const c = value as Record<string, unknown>;
  if (typeof c.op !== "string" || !COMPARE_OPS.includes(c.op as PredicateCompareOp)) {
    throw new Error(`${factory}: \`${field}.compare.op\` must be one of greaterThan, lessThan, atLeast, atMost`);
  }
  if (typeof c.value !== "number" || !Number.isFinite(c.value)) {
    throw new Error(`${factory}: \`${field}.compare.value\` must be a finite number`);
  }
  return { op: c.op as PredicateCompareOp, value: c.value };
}
