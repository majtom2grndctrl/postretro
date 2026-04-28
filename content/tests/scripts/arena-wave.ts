import { registerHandler } from "postretro";
import { world } from "../../../sdk/lib/world";

registerHandler("levelLoad", () => {
  setupArena1Wave();
  setupArena2Wave();
});

function setupArena1Wave() {
  const lights = world.query({ component: "light", tag: "arena_1_light" });

  if (lights.length === 0) return;

  // Calculate centroid position
  let centroidX = 0,
    centroidZ = 0;
  for (const light of lights) {
    centroidX += light.transform.position.x;
    centroidZ += light.transform.position.z;
  }
  centroidX /= lights.length;
  centroidZ /= lights.length;

  // Compute angle of each light around centroid
  const lightsWithAngle = lights.map((light) => {
    const dx = light.transform.position.x - centroidX;
    const dz = light.transform.position.z - centroidZ;
    const angle = Math.atan2(dz, dx);
    return { light, angle };
  });

  // Anchor the wave to the NW corner: the westernmost light has the highest
  // position.z (engine z = -map_x, so lowest map X = highest z).
  const startAngle = lightsWithAngle.reduce((best, cur) =>
    cur.light.transform.position.z > best.light.transform.position.z
      ? cur
      : best,
  ).angle;

  // Sort counterclockwise from NW corner so the wave sweeps left-to-right
  // across the north wall then down the east side.
  const TWO_PI = 2 * Math.PI;
  lightsWithAngle.sort((a, b) => {
    const da = (a.angle - startAngle + TWO_PI) % TWO_PI;
    const db = (b.angle - startAngle + TWO_PI) % TWO_PI;
    return da - db;
  });

  // Build brightness curve: half-sine pulse followed by silence for the rest of the period
  const pulseDurationMs = 300;
  const lightSpacingMs = 150;
  const cyclePauseMs = 2000;

  const numLights = lightsWithAngle.length;
  const periodMs =
    (numLights - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
  const pulseFraction = pulseDurationMs / periodMs;

  const samples = 32;
  const brightness = [];
  for (let i = 0; i < samples; i++) {
    const t = i / samples;
    brightness.push(
      t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
    );
  }

  // Apply animation with phase offset for wave effect
  for (let i = 0; i < lightsWithAngle.length; i++) {
    const phase = (i * lightSpacingMs) / periodMs;

    lightsWithAngle[i].light.setAnimation({
      periodMs,
      phase,
      playCount: null,
      startActive: true,
      brightness,
      color: null,
      direction: null,
    });
  }
}

function setupArena2Wave() {
  const lights = world.query({ component: "light", tag: "arena_wave_2" });

  if (lights.length === 0) return;

  // Lights are on the west wall; the wave runs south → north.
  // Swizzle: engine_x = -quake_y, so south (low Quake Y) = high engine X.
  // Sort descending by position.x so index 0 is the southernmost light.
  const sorted = [...lights].sort(
    (a, b) => b.transform.position.x - a.transform.position.x,
  );

  const pulseDurationMs = 200;
  const lightSpacingMs = 50;
  const cyclePauseMs = 2000;

  const numLights = sorted.length;
  const periodMs =
    (numLights - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
  const pulseFraction = pulseDurationMs / periodMs;

  const samples = 32;
  const brightness = [];
  for (let i = 0; i < samples; i++) {
    const t = i / samples;
    brightness.push(
      t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
    );
  }

  for (let i = 0; i < sorted.length; i++) {
    const phase = (i * lightSpacingMs) / periodMs;

    sorted[i].setAnimation({
      periodMs,
      phase,
      playCount: null,
      startActive: true,
      brightness,
      color: null,
      direction: null,
    });
  }
}
