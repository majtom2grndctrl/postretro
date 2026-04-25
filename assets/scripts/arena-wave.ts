import { registerHandler } from "postretro";
import { world } from "./sdk/world";

registerHandler("levelLoad", () => {
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

  // Sort lights by clockwise angle around centroid
  const lightsWithAngle = lights.map((light) => {
    const dx = light.transform.position.x - centroidX;
    const dz = light.transform.position.z - centroidZ;
    const angle = Math.atan2(dz, dx);
    return { light, angle };
  });

  lightsWithAngle.sort((a, b) => a.angle - b.angle);

  // Build brightness curve: half-sine pulse followed by silence for the rest of the period
  const pulseDurationMs = 600;
  const lightSpacingMs = 300;
  const cyclePauseMs = 5000;

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
});
