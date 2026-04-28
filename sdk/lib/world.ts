// World-query vocabulary: typed wrapper around `worldQuery` plus the
// `LightEntity` handle methods. The primitive does the actual work; this
// file exists so modders import `world` and `LightEntity` the same way
// they import other vocabulary helpers, and so the handle's convenience
// methods (`setAnimation`, `setIntensity`, `setColor`) have a clear home.
//
// ---------------------------------------------------------------------------
// Canonical modder example — rolling pulse down a hallway, 10s loop.
//
// Map authors tag the hallway lights `"hallway_wave"` in TrenchBroom. The
// behavior script below queries them at level load, sorts along the x
// axis, and staggers `phase` across the row so the pulse travels.
//
// ```typescript
// import { registerHandler } from "postretro";
// import { world } from "./world";
// import type { LightAnimation } from "postretro";
//
// registerHandler("levelLoad", () => {
//   const lights = world
//     .query({ component: "light", tag: "hallway_wave" })
//     .sort((a, b) => a.transform.position.x - b.transform.position.x);
//
//   const pulse: LightAnimation = {
//     periodMs: 10000,
//     brightness: [
//       0.1, 0.1, 0.1, 0.1, 0.1,
//       0.3, 0.8, 1.0, 0.8, 0.3,
//       0.1, 0.1, 0.1, 0.1, 0.1,
//       0.1, 0.1, 0.1, 0.1, 0.1,
//     ],
//   };
//
//   lights.forEach((light, i) => {
//     light.setAnimation({ ...pulse, phase: i / lights.length });
//   });
// });
// ```
// ---------------------------------------------------------------------------

import {
  getComponent,
  setLightAnimation,
  worldQuery,
} from "postretro";
import type {
  Entity,
  EntityId,
  LightAnimation,
  LightComponent,
  LightEntity as GeneratedLightEntity,
  Vec3,
  WorldQueryFilter,
} from "postretro";

/** Easing family used by `setIntensity` / `setColor` transitions. */
export type EasingCurve = "linear" | "easeIn" | "easeOut" | "easeInOut";

/**
 * Typed handle returned by `world.query` for a light entity. Wraps the
 * generated `LightEntity` snapshot with convenience methods that call the
 * underlying scripting primitives.
 */
export interface LightEntity extends GeneratedLightEntity {
  /**
   * Replace the light's animation. Pass `null` to clear it. Last call
   * wins — lights are always interruptible.
   */
  setAnimation(anim: LightAnimation | null): void;

  /**
   * Transition the light's intensity to `target` over `transitionMs`
   * milliseconds (default `0`, which applies the target instantly).
   *
   * Re-reads the live `LightComponent.intensity` from the registry at
   * call time rather than the handle's query-time snapshot, so chained
   * transitions compose correctly. `easing` defaults to `"easeInOut"`
   * when `transitionMs > 0`; it is ignored for instant transitions.
   *
   * Internally constructs a one-cycle `LightAnimation` (`playCount: 1`)
   * and hands it to `setAnimation`.
   */
  setIntensity(
    target: number,
    transitionMs?: number,
    easing?: EasingCurve,
  ): void;

  /**
   * Transition the light's RGB color to `target` over `transitionMs`
   * milliseconds (default `0`). Same live-read / one-cycle pattern as
   * `setIntensity`.
   *
   * Throws a descriptive `Error` on non-dynamic lights (baked lights
   * cannot have their color animated because their indirect SH was
   * baked with the compile-time color).
   */
  setColor(
    target: [number, number, number],
    transitionMs?: number,
    easing?: EasingCurve,
  ): void;
}

/**
 * Maps a component-name string literal to the rich entity handle type
 * returned by `world.query`. Extend this as new component types gain
 * dedicated handles; unknown component names fall back to `Entity`.
 */
export type EntityForComponent<T extends string> =
  T extends "light" ? LightEntity : Entity;

/** Typed vocabulary object returned from `world.query`. */
export interface World {
  /**
   * Query entities matching the filter. The return type is selected by
   * the literal `component` string: `"light"` yields `LightEntity[]`
   * (with convenience methods `setAnimation`, `setIntensity`,
   * `setColor`); any other component name yields base `Entity[]`
   * (id, transform, tag) — use `getComponent` to access component data.
   *
   * **Note:** Unknown `component` strings (e.g. typos like `"lights"`)
   * compile without error but throw `InvalidArgument` at runtime; only
   * `"light"` is supported in the current build.
   */
  query<T extends string>(
    filter: { component: T; tag?: string | null },
  ): EntityForComponent<T>[];
}

export const world: World = {
  query<T extends string>(
    filter: { component: T; tag?: string | null },
  ): EntityForComponent<T>[] {
    const normalized: WorldQueryFilter = {
      component: filter.component,
      tag: filter.tag ?? null,
    };
    const raw = worldQuery(normalized);
    if (filter.component === "light") {
      const lights = (raw as ReadonlyArray<GeneratedLightEntity>).map(
        wrapLightEntity,
      );
      return lights as EntityForComponent<T>[];
    }
    // Project per-component snapshots down to the `Entity` shape so
    // callers using the generic path don't observe component-specific
    // fields that the type does not promise.
    const entities: Entity[] = raw.map((s) => ({
      id: s.id,
      transform: s.transform,
      tag: s.tag ?? null,
    }));
    return entities as EntityForComponent<T>[];
  },
};

// ---------------------------------------------------------------------------
// Handle construction
// ---------------------------------------------------------------------------

function wrapLightEntity(snapshot: GeneratedLightEntity): LightEntity {
  const id: EntityId = snapshot.id;

  const handle: LightEntity = {
    ...snapshot,

    setAnimation(anim: LightAnimation | null): void {
      if (anim && anim.color && !snapshot.isDynamic) {
        throw new Error(
          `setAnimation: light ${idDebug(id)} is not dynamic; color animation is only valid on dynamic lights`,
        );
      }
      setLightAnimation(id, anim);
    },

    setIntensity(
      target: number,
      transitionMs: number = 0,
      easing?: EasingCurve,
    ): void {
      const live = readLightComponent(id);
      const anim = buildIntensityAnimation(
        live.intensity,
        target,
        transitionMs,
        easing,
      );
      setLightAnimation(id, anim);
    },

    setColor(
      target: [number, number, number],
      transitionMs: number = 0,
      easing?: EasingCurve,
    ): void {
      if (!snapshot.isDynamic) {
        throw new Error(
          `setColor: light ${idDebug(id)} is not dynamic; color can only be animated on dynamic lights`,
        );
      }
      const live = readLightComponent(id);
      const anim = buildColorAnimation(
        live.color,
        { x: target[0], y: target[1], z: target[2] },
        transitionMs,
        easing,
      );
      setLightAnimation(id, anim);
    },
  };

  return handle;
}

function readLightComponent(id: EntityId): LightComponent {
  const c = getComponent(id, "Light");
  if (c.kind !== "Light") {
    throw new Error(
      `expected Light component on entity ${idDebug(id)}, got ${c.kind}`,
    );
  }
  return c.value;
}

function idDebug(id: EntityId): string {
  // `EntityId` is a branded number — print the underlying value for
  // error messages without leaking the brand in the type.
  return String(id as unknown as number);
}

// ---------------------------------------------------------------------------
// Easing + one-cycle animation builders
// ---------------------------------------------------------------------------

// 8-sample resolution for transitions. Fine enough for a smooth ease
// without bloating the primitive call payload.
const EASE_SAMPLES = 8;

function resolveEasing(
  transitionMs: number,
  easing: EasingCurve | undefined,
): EasingCurve {
  if (transitionMs <= 0) {
    // Irrelevant for step transitions, but pick a stable default.
    return "linear";
  }
  return easing ?? "easeInOut";
}

function easeAt(curve: EasingCurve, t: number): number {
  // t is the normalized sample position in [0, 1].
  switch (curve) {
    case "linear":
      return t;
    case "easeIn":
      return t * t;
    case "easeOut":
      return 1 - (1 - t) * (1 - t);
    case "easeInOut":
      // Smoothstep-style ease-in-out.
      return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
  }
}

function buildIntensityAnimation(
  from: number,
  to: number,
  transitionMs: number,
  easing: EasingCurve | undefined,
): LightAnimation {
  if (transitionMs <= 0) {
    return {
      periodMs: 1,
      phase: null,
      playCount: 1,
      brightness: [to],
      color: null,
      direction: null,
    };
  }
  const curve = resolveEasing(transitionMs, easing);
  const brightness: number[] = new Array(EASE_SAMPLES);
  for (let i = 0; i < EASE_SAMPLES; i++) {
    const t = i / (EASE_SAMPLES - 1);
    brightness[i] = from + (to - from) * easeAt(curve, t);
  }
  return {
    periodMs: transitionMs,
    phase: null,
    playCount: 1,
    brightness,
    color: null,
    direction: null,
  };
}

function buildColorAnimation(
  from: Vec3,
  to: Vec3,
  transitionMs: number,
  easing: EasingCurve | undefined,
): LightAnimation {
  if (transitionMs <= 0) {
    return {
      periodMs: 1,
      phase: null,
      playCount: 1,
      brightness: null,
      color: [{ x: to.x, y: to.y, z: to.z }],
      direction: null,
    };
  }
  const curve = resolveEasing(transitionMs, easing);
  const color: Vec3[] = new Array(EASE_SAMPLES);
  for (let i = 0; i < EASE_SAMPLES; i++) {
    const t = i / (EASE_SAMPLES - 1);
    const k = easeAt(curve, t);
    color[i] = {
      x: from.x + (to.x - from.x) * k,
      y: from.y + (to.y - from.y) * k,
      z: from.z + (to.z - from.z) * k,
    };
  }
  return {
    periodMs: transitionMs,
    phase: null,
    playCount: 1,
    brightness: null,
    color,
    direction: null,
  };
}
