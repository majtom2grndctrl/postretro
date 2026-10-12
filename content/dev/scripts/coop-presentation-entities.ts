// Co-op fixture descriptors. Registered by the dev mod; map data scripts
// cannot declare entities. All models and sprites are already shipped in dev.
import { activation, defineEntity, defineImpactEvent } from "postretro";
import { combatDummyLifecycle } from "./combat-lifecycle";

const KNIGHT = "models/reference_enemy_kaykit_knight/scene.gltf";
const SMG = "models/cyberpunk_weapons/smg/model.gltf";
const light = { color: [0.2, 0.8, 1] as [number, number, number], intensity: 8, range: 6, is_dynamic: true };
const animatedMesh = {
  model: KNIGHT,
  animations: {
    idle: { clip: "Idle", loop: true },
    alert: { clip: "Walking_A", loop: true, crossfadeMs: 200 },
    death: { clip: "Death_A", loop: false, crossfadeMs: 120, interrupt: "snap" as const },
  },
  defaultState: "idle",
};

export const coopPresentationEntities = [
  defineEntity({ canonicalName: "coop_light_only", components: { light } }),
  defineEntity({ canonicalName: "coop_lit_prop", components: { light, mesh: animatedMesh } }),
  defineEntity({ canonicalName: "coop_mesh_prop", components: { mesh: animatedMesh } }),
  defineEntity({ canonicalName: "coop_emitter_only", components: {
    emitter: {
      rate: 14, spread: 0.2, lifetime: 4, velocity: { x: 0, y: 0.8, z: 0 }, buoyancy: 0.5, drag: 0.5,
      size_over_lifetime: [1], opacity_over_lifetime: [1, 0], color: { x: 1, y: 1, z: 1 },
      sprite: "smoke_puff", spin_rate: 0.5,
    },
  } }),
  defineEntity({ canonicalName: "coop_target_dummy", components: {
    mesh: animatedMesh,
    health: { max: 48, hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] } },
  } }),
  defineEntity({ canonicalName: "coop_invisible_target", components: {
    health: { max: 48, hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] } },
  } }),
  defineEntity({ canonicalName: "coop_health_weapon", components: {
    health: { max: 48, hitbox: { halfExtents: [0.3, 0.2, 0.5], offset: [0, 0.2, 0] } },
    mesh: { model: SMG }, touchable: { mode: "press", radius: 1 },
    weapon: {
      damage: 8, range: 48, resolution: "hitscan", thirdPersonModel: SMG, viewmodel: SMG,
      muzzleOffset: [0, 0.274, -0.567],
      primary: { trigger: "press", recoveryMs: 240, steps: [activation.shot()] },
      resource: { kind: "ammo", type: "bullets.coop_fixture", magazine: 9, reserve: 27, reloadMs: 450, reloadStyle: "magazine" },
      sounds: { fire: "weapons/pistol_fire", dryFire: "weapons/dry_fire" },
    },
  } }),
  defineEntity({ canonicalName: "coop_heat_test", components: {
    mesh: { model: SMG }, touchable: { mode: "press", radius: 1 },
    weapon: {
      damage: 1, range: 48, resolution: "hitscan", thirdPersonModel: SMG, viewmodel: SMG,
      muzzleOffset: [0, 0.274, -0.567],
      primary: { trigger: "hold", recoveryMs: 150, steps: [activation.shot()] },
      resource: { kind: "heat", heatPerShot: 25, overheatAt: 100, coolPerSecond: 20, coolDelayMs: 500 },
      sounds: { fire: "weapons/pistol_fire" },
    },
  } }),
];

// Preserve the existing dummy XP payout, while this map's dummy specialization
// plays a real death clip and leaves a short, observable corpse on both peers.
export const coopDummyLifecycle = combatDummyLifecycle.override({ tag: "coop_dummy" }, (impact) => {
  const target = impact.target;
  const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
  return [{ when: killed, do: [target.playAnim("death"), target.despawn({ afterMs: 1800 })] }];
});

// A retained-Health check with a visible endpoint: after the one-use 12-damage
// pad, three 12-damage pistol hits after pickup/holster/drop remove the item.
export const coopWeaponLifecycle = defineImpactEvent("coop-weapon-retained", { tag: "coop_health_weapon" }, (impact) => [{
  when: impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0)),
  do: [impact.target.despawn()],
}]);
