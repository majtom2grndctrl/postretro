// Attack-token declarations: `defineAttackToken`, the SDK's `defaultToken`, and
// `token.with(...)` references. Pure data; a token counts only once
// `defineMod({ attackTokens })` registers it.
// See: context/lib/scripting.md §2 (Attack token declarations)

type AttackToken = import("postretro").AttackToken;
type AttackTokenBudget = import("postretro").AttackTokenBudget;
type AttackTokenFields = import("postretro").AttackTokenFields;

const ATTACK_TOKEN_NAME_DIAGNOSTIC = "defineAttackToken without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration";

// Declarations `defineMod({ attackTokens })` resolves each handle through.
const attackTokenDeclarations = new WeakMap<object, object>();

// A handle crosses as `{ kind, name }`; `with` stays off the wire.
function attackTokenHandle(name: string): AttackToken {
  const handle = { kind: "attackToken", name };
  Object.defineProperty(handle, "with", {
    value: (fields: AttackTokenFields) => Object.freeze({ ...fields, token: handle }),
    enumerable: false,
    configurable: false,
    writable: false,
  });
  return Object.freeze(handle) as unknown as AttackToken;
}

/** The SDK's attack token at the default budget: what an attack without `draws` draws. Every mod has it and never lists it in `attackTokens`; a profile may override it. */
export const defaultToken: AttackToken = attackTokenHandle("default");

/** Declare an attack token by its default budget: `limit` (default 3) is how much of it may be drawn on one target at once, `refreshMs` (default 0) how long a spent draw stays counted after its turn ends. Omit `name` only in a TypeScript direct top-level binding declaration compiled by scripts-build, which names it by its binding. Register it with `defineMod({ attackTokens: [token] })`. */
export function defineAttackToken(budget?: AttackTokenBudget): AttackToken;
export function defineAttackToken(name: string, budget?: AttackTokenBudget): AttackToken;
export function defineAttackToken(
  nameOrBudget?: string | AttackTokenBudget,
  budget?: AttackTokenBudget,
): AttackToken {
  if (typeof nameOrBudget !== "string") throw new TypeError(ATTACK_TOKEN_NAME_DIAGNOSTIC);
  const handle = attackTokenHandle(nameOrBudget);
  attackTokenDeclarations.set(
    handle,
    Object.freeze({ kind: "attackToken", name: nameOrBudget, budget: { ...(budget ?? {}) } }),
  );
  return handle;
}

/** The declaration an `attackTokens` entry carries across the manifest. A bare string, a handle of another kind or `defaultToken` passes through, and the engine rejects it naming the list entry. */
export function attackTokenDeclarationOf(entry: unknown): unknown {
  if (entry === null || typeof entry !== "object") return entry;
  return attackTokenDeclarations.get(entry) ?? entry;
}
