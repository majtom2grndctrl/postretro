// Attack and stance declarations: `defineAttack` and `defineStance`. A handle
// carries its whole declaration, so a behavior that references it needs no
// separate registration; load-time lowering in the engine resolves handles to
// the attack names the runtime descriptor uses.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

type Attack = import("postretro").Attack;
type AttackDeclaration = import("postretro").AttackDeclaration;
type Stance = import("postretro").Stance;
type StanceOptions = import("postretro").StanceOptions;

const ATTACK_NAME_DIAGNOSTIC = "defineAttack without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration";
const STANCE_NAME_DIAGNOSTIC = "defineStance without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration";

/** Declare an attack: its engine fields (`damage`, `maxRange`, `cooldownMs`, `draws`, ... or `weapon`) plus the stance it begins and ends in and its `release` and `recover`. Omit `name` only in a TypeScript direct top-level binding declaration compiled by scripts-build, which names it by its binding. Reference the handle wherever an attack is meant: `action: { attack }`, `attackTurn`, a behavior's `attacks` list, `attackTurnFree`, `attackTurnHeld`. */
export function defineAttack(declaration: AttackDeclaration): Attack;
export function defineAttack(name: string, declaration: AttackDeclaration): Attack;
export function defineAttack(
  nameOrDeclaration: string | AttackDeclaration,
  declaration?: AttackDeclaration,
): Attack {
  if (typeof nameOrDeclaration !== "string") throw new TypeError(ATTACK_NAME_DIAGNOSTIC);
  return Object.freeze({ ...(declaration ?? {}), kind: "attack", name: nameOrDeclaration }) as unknown as Attack;
}

/** Declare a stance: a pose an enemy rests in before an attack that names it as its `beginningStance`. Omit `name` only in a TypeScript direct top-level binding declaration compiled by scripts-build, which names it by its binding. */
export function defineStance(options: StanceOptions): Stance;
export function defineStance(name: string, options: StanceOptions): Stance;
export function defineStance(
  nameOrOptions: string | StanceOptions,
  options?: StanceOptions,
): Stance {
  if (typeof nameOrOptions !== "string") throw new TypeError(STANCE_NAME_DIAGNOSTIC);
  return Object.freeze({ ...(options ?? {}), kind: "stance", name: nameOrOptions }) as unknown as Stance;
}
