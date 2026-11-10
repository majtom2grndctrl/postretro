// The dev mod's attack tokens: what each enemy attack draws from its target's
// allowance. Each object is the token's default budget; `start-script.ts`
// registers all three and declares the `arena` profile over them.
// See: docs/scripting-reference.md (Attack tokens and budgets)

import { defineAttackToken } from "postretro";

// Up to three melee draws on one target at once: a slam counts two, a jab or
// a claw one.
export const melee = defineAttackToken({ limit: 3 });
// Two rifle bursts on one target at once; the rest of a squad strafes.
export const ranged = defineAttackToken({ limit: 2 });
// One heavy attack on one target, then shut for 4 s after its turn ends.
export const heavy = defineAttackToken({ limit: 1, refreshMs: 4000 });
