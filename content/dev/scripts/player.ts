import { registerEntity } from "postretro";

registerEntity({
  classname: "player",
  components: {
    movement: {
      capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: 7.0,
        accel: 10.0,
        jumpVelocity: 5.5,
        stepHeight: 0.3,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.0,
        accel: 0.7,
        maxControlSpeed: 1.0,
        bunnyHop: false,
        jumps: 0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 40.0 },
    },
  },
});
