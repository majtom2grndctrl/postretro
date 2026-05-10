import {
  fogPulse,
  type NamedReactionDescriptor,
  registerEntity,
  registerReaction,
  world,
} from "postretro";

export function registerLevelManifest(_ctx: unknown) {
  // Reference behavior archetype registrations. Components are intentionally
  // empty: both archetypes are pure tag/transform carriers and the
  // behavior scripts (`rotator-driver.ts`, `damage-source.ts`) locate
  // their work via tag-filtered `worldQuery` rather than component data.
  registerEntity({
    classname: "game_rotator_driver",
  });
  registerEntity({
    classname: "game_damage_source",
  });

  const reactions: NamedReactionDescriptor[] = [];

  // Arena 1: angular sweep from the NW corner, counterclockwise.
  const arena1Raw = world.query({ component: "light", tag: "arena_1_light" });
  if (arena1Raw.length > 0) {
    let centroidX = 0,
      centroidZ = 0;
    for (const light of arena1Raw) {
      centroidX += light.position.x;
      centroidZ += light.position.z;
    }
    centroidX /= arena1Raw.length;
    centroidZ /= arena1Raw.length;

    const lightsWithAngle = arena1Raw.map((light) => {
      const dx = light.position.x - centroidX;
      const dz = light.position.z - centroidZ;
      return { light, angle: Math.atan2(dz, dx) };
    });

    // Anchor at the NW corner: the light with the highest z (westernmost).
    const startAngle = lightsWithAngle.reduce((best, cur) =>
      cur.light.position.z > best.light.position.z ? cur : best,
    ).angle;

    const TWO_PI = 2 * Math.PI;
    lightsWithAngle.sort((a, b) => {
      const da = (a.angle - startAngle + TWO_PI) % TWO_PI;
      const db = (b.angle - startAngle + TWO_PI) % TWO_PI;
      return da - db;
    });

    const pulseDurationMs = 300;
    const lightSpacingMs = 150;
    const cyclePauseMs = 2000;
    const N = lightsWithAngle.length;
    const periodMs = (N - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
    const pulseFraction = pulseDurationMs / periodMs;

    const SAMPLES = 32;
    const brightness: number[] = [];
    for (let i = 0; i < SAMPLES; i++) {
      const t = i / SAMPLES;
      brightness.push(
        t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
      );
    }

    const steps = lightsWithAngle.map(({ light }, i) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs,
        phase: (i * lightSpacingMs) / periodMs,
        playCount: null,
        startActive: true,
        brightness,
        color: null,
        direction: null,
      },
    }));

    reactions.push(registerReaction("levelLoad", { sequence: steps }));
  }

  // Arena 2: west-wall wave, south → north (descending engine-x order).
  const arena2Raw = world.query({ component: "light", tag: "arena_wave_2" });
  if (arena2Raw.length > 0) {
    const sorted = [...arena2Raw].sort((a, b) => b.position.x - a.position.x);

    const pulseDurationMs = 200;
    const lightSpacingMs = 50;
    const cyclePauseMs = 2000;
    const N = sorted.length;
    const periodMs = (N - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
    const pulseFraction = pulseDurationMs / periodMs;

    const SAMPLES = 32;
    const brightness: number[] = [];
    for (let i = 0; i < SAMPLES; i++) {
      const t = i / SAMPLES;
      brightness.push(
        t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
      );
    }

    const steps = sorted.map((light, i) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs,
        phase: (i * lightSpacingMs) / periodMs,
        playCount: null,
        startActive: true,
        brightness,
        color: null,
        direction: null,
      },
    }));

    reactions.push(registerReaction("levelLoad", { sequence: steps }));
  }

  // Fog demo: both fog entity types in the map carry the "pulse_fog" tag,
  // so the tag-targeted scatter primitive and the per-id fogPulse sequence
  // both demonstrate cross-subtype dispatch (fog_volume + fog_lamp hit together).
  const fogs = world.query({ component: "fog_volume", tag: "pulse_fog" });
  if (fogs.length > 0) {
    // Tag-targeted Primitive: one descriptor, batch-applied to every
    // "pulse_fog" volume regardless of entity subtype.
    reactions.push(
      registerReaction("levelLoad", {
        primitive: "setFogScatter",
        tag: "pulse_fog",
        args: { scatter: 0.4 },
      }),
    );

    // Per-id Sequence: a single `setFogAnimation` step carrying a sine
    // density curve, evaluated per-frame across `periodMs` on each
    // matched volume.
    for (const fog of fogs) {
      const steps = fogPulse(fog.id, 0.2, 1.0, 1.5);
      reactions.push(registerReaction("levelLoad", { sequence: steps }));
    }
  }

  return { reactions };
}
