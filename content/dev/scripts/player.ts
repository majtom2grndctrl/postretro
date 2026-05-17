import { defineEntity } from "postretro";

export const playerEntity = defineEntity({
  components: {
    movement: {
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: 10.0,
        accel: 10.0,
        jumpVelocity: 9,
        stepHeight: 0.4,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.5,
        accel: 2,
        maxControlSpeed: 2,
        bunnyHop: true,
        jumps: 0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 40.0 },
    },
  },
});
