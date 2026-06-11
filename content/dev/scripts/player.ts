import { defineEntity } from "postretro";

export const playerEntity = defineEntity({
  canonicalName: "player",
  defaultWeapon: "reference_pistol",
  components: {
    movement: {
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: { walk: 7.0, run: 11.0, crouch: 3.0 },
        accel: 8.0,
        stepHeight: 0.5,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.5,
        accel: 10,
        maxControlSpeed: 2,
        bunnyHop: true,
        jumps: 0,
        jumpVelocity: 9,
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
      crouch: {
        halfHeight: 0.4,
        eyeHeight: 0.3,
        transitionRate: 8.0,
      },
      viewFeel: {
        bob: {
          frequency: 0.25,
          verticalAmplitude: 0.1,
          lateralAmplitude: 0,
          speedThreshold: 2.0,
        },
        tilt: {
          speedReference: 10,
          maxAngle: 4,
          tension: 15,
        },
      },
    },
  },
});
