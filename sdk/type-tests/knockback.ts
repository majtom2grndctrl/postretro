import type { KnockbackDescriptor, SplashKnockbackDescriptor, KnockbackResponse, WeaponDescriptor, PlayerMovementDescriptor, BehaviorGraphDescriptor } from "postretro";

const direct = { speed: 8, upwardBias: 0.2 } satisfies KnockbackDescriptor;
const splash = { speed: 14, selfScale: 1, minFraction: 0.25 } satisfies SplashKnockbackDescriptor;
const response = { scale: 0.5, groundDrag: 8, airDrag: 0, control: 0.25 } satisfies KnockbackResponse;
const defaults = {} satisfies KnockbackResponse;
const weaponPush: Pick<WeaponDescriptor, "knockback" | "splash"> = { knockback: direct, splash: { radius: 6, selfDamage: false, knockback: splash } };
const playerResponse: Pick<PlayerMovementDescriptor, "knockback"> = { knockback: response };
const enemyResponse: Pick<BehaviorGraphDescriptor, "knockback"> = { knockback: defaults };
// @ts-expect-error Push speed is required when a hit impulse is authored.
const missingSpeed: KnockbackDescriptor = { upwardBias: 0.5 };
// @ts-expect-error Authoring keys use camelCase.
const wrongResponse: KnockbackResponse = { ground_drag: 8 };
void [weaponPush, playerResponse, enemyResponse, missingSpeed, wrongResponse];
