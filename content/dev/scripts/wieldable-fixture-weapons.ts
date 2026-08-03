import { defineEntity } from "postretro";

const FIXTURE_MODEL = "models/smg/model.gltf";

export const wieldableFixtureAutoEntity = defineEntity({
  canonicalName: "wieldable_fixture_auto",
  components: {
    weapon: {
      damage: 8.0,
      range: 48.0,
      fireRateMs: 240.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: FIXTURE_MODEL,
      viewmodel: FIXTURE_MODEL,
      resource: {
        kind: "ammo",
        type: "bullets.fixture_auto",
        magazine: 9,
        reserve: 27,
        reloadMs: 450,
        reloadStyle: "magazine",
      },
    },
    mesh: { model: FIXTURE_MODEL },
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
      thirdPersonModel: FIXTURE_MODEL,
      viewmodel: FIXTURE_MODEL,
      resource: {
        kind: "ammo",
        type: "shells.fixture_press",
        magazine: 5,
        reserve: 15,
        reloadMs: 550,
        reloadStyle: "perShell",
      },
    },
    mesh: { model: FIXTURE_MODEL },
    touchable: { mode: "press", radius: 1.0 },
  },
});
