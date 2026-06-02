import { defineEntity } from "postretro";

export const playerEntity = defineEntity({
  canonicalName: "player",
  defaultWeapon: "reference_pistol",
  components: {
    movement: {
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: { walk: 7.0, run: 11.0 },
        accel: 8.0,
        jumpVelocity: 9,
        stepHeight: 0.5,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.5,
        accel: 10,
        maxControlSpeed: 2,
        bunnyHop: true,
        jumps: 0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 40.0 },
      dash: {
        boostSpeed: 22.0,
        momentumRetention: 0.5,
        steerControl: 0.3,
        dashDrag: 0,
        cooldownMs: 600,
        airDashes: 1,
        preserveVertical: false,
      },
    },
  },
});
