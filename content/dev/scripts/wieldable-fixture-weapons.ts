import { defineEntity } from "postretro";

const SMG_MODEL = "models/cyberpunk_weapons/smg/model.gltf";
const SHOTGUN_MODEL = "models/cyberpunk_weapons/shotgun/model.gltf";

export const wieldableFixtureAutoEntity = defineEntity({
  canonicalName: "wieldable_fixture_auto",
  components: {
    weapon: {
      damage: 8.0,
      range: 48.0,
      fireRateMs: 240.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: SMG_MODEL,
      viewmodel: SMG_MODEL,
      // Authored from the SMG viewmodel's rigid `muzzle` socket.
      muzzleOffset: [0.0, 0.274, -0.567],
      resource: {
        kind: "ammo",
        type: "bullets.fixture_auto",
        magazine: 9,
        reserve: 27,
        reloadMs: 450,
        reloadStyle: "magazine",
      },
    },
    mesh: { model: SMG_MODEL },
    touchable: { mode: "auto", radius: 1.0 },
  },
});

export const wieldableFixturePressEntity = defineEntity({
  canonicalName: "wieldable_fixture_press",
  components: {
    weapon: {
      damage: 18.0,
      range: 48.0,
      fireRateMs: 600.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: SHOTGUN_MODEL,
      viewmodel: SHOTGUN_MODEL,
      // Authored from the shotgun viewmodel's rigid `muzzle` socket.
      muzzleOffset: [0.0, 0.34, -1.137],
      resource: {
        kind: "ammo",
        type: "shells.fixture_press",
        magazine: 5,
        reserve: 15,
        reloadMs: 550,
        reloadStyle: "perShell",
      },
    },
    mesh: { model: SHOTGUN_MODEL },
    touchable: { mode: "press", radius: 1.0 },
  },
});
