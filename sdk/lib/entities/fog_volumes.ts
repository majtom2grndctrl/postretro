// Fog volume entity handle and animation constructors.
// Mirrors the LightEntity vocabulary in `./lights`, adapted to the
// fog_volume ComponentValue surface.
// See: context/lib/scripting.md §10 (external API shape)
// and docs/scripting-reference.md (public API surface).

import {
  getComponent,
  registerHandler,
  setComponent,
} from "postretro";
import type {
  EntityId,
  FogVolumeComponent,
  FogVolumeEntity as GeneratedFogVolumeEntity,
  ScriptCallContext,
} from "postretro";

/**
 * Typed handle returned by `world.query({ component: "fog_volume" })`.
 * Wraps the generated `FogVolumeEntity` snapshot with mutating methods
 * that call `setComponent` directly. Density and color tweens are
 * driven from a per-handle `tick` callback registered with
 * `registerHandler` — there is no engine-side fog animation primitive,
 * unlike lights.
 */
export interface FogVolumeHandle extends GeneratedFogVolumeEntity {
  /**
   * Set fog density. Instant when `durationMs` is `0` or omitted;
   * otherwise lerped each tick from the live density value over
   * `durationMs` milliseconds. Last call wins — the previous tween
   * (if any) is cancelled.
   */
  setDensity(density: number, durationMs?: number): void;

  /**
   * Set fog color. Instant when `durationMs` is `0` or omitted;
   * otherwise lerped each tick from the live color over `durationMs`
   * milliseconds. Last call wins.
   */
  setColor(color: [number, number, number], durationMs?: number): void;

  /** Set the scatter fraction. Instant. */
  setScatter(scatter: number): void;

  /**
   * Set the edge softness (world units). Instant.
   *
   * Note: `edge_softness` only affects brush `fog_volume` entities; it is
   * ignored by the shader for `fog_lamp` and `fog_tube`.
   */
  setEdgeSoftness(edgeSoftness: number): void;
}

/** Controller returned by `pulseDensity` to cancel the running tick handler. */
export interface AnimationController {
  /** Stop the animation. Idempotent — subsequent calls are no-ops. */
  stop(): void;
}

function readFogVolumeComponent(id: EntityId): FogVolumeComponent {
  const c = getComponent(id, "fog_volume");
  if (c.kind !== "fog_volume") {
    throw new Error(
      `expected FogVolume component on entity ${idDebug(id)}, got ${c.kind}`,
    );
  }
  // Flat ComponentValue: `kind` plus the FogVolumeComponent fields.
  return c as unknown as FogVolumeComponent;
}

function idDebug(id: EntityId): string {
  return String(id as unknown as number);
}

/**
 * A cancelable tick subscription. `registerHandler` itself has no
 * unregister primitive, so we wrap the user callback in a closure that
 * checks a `cancelled` flag on each tick — the handler stays installed
 * for the life of the level but becomes a no-op once stopped.
 */
function tickSubscription(
  fn: (ctx: ScriptCallContext) => void,
): AnimationController {
  let cancelled = false;
  registerHandler("tick", (ctx?: ScriptCallContext) => {
    if (cancelled) return;
    if (ctx === undefined) return;
    fn(ctx);
  });
  return {
    stop(): void {
      cancelled = true;
    },
  };
}

/**
 * Per-handle slot that owns the currently-running density tween (if
 * any). Stored on the handle as a non-enumerable property so chained
 * calls cancel the previous tween before starting a new one.
 */
const DENSITY_TWEEN = Symbol("fog_density_tween");
const COLOR_TWEEN = Symbol("fog_color_tween");

interface TweenSlots {
  [DENSITY_TWEEN]?: AnimationController | null;
  [COLOR_TWEEN]?: AnimationController | null;
}

function cancelExisting(slots: TweenSlots, key: symbol): void {
  const slotKey = key as keyof TweenSlots;
  const existing = slots[slotKey];
  if (existing) {
    existing.stop();
    slots[slotKey] = null;
  }
}

function writeFogVolume(
  id: EntityId,
  density: number,
  color: [number, number, number],
  scatter: number,
  edgeSoftness: number,
): void {
  // `kind` is required: the scripting runtime's `FromJs for ComponentValue`
  // reads the `kind` field first to dispatch to the correct component branch
  // (see `scripting/conv.rs`). Omitting it would produce "missing field `kind`"
  // at the Rust boundary and silently drop the setComponent call.
  setComponent(id, "fog_volume", {
    kind: "fog_volume",
    density,
    color: [color[0], color[1], color[2]],
    scatter,
    edge_softness: edgeSoftness,
  } as unknown as ComponentValuePayload);
}

function startDensityTween(
  id: EntityId,
  slots: TweenSlots,
  target: number,
  durationMs: number,
): void {
  cancelExisting(slots, DENSITY_TWEEN);
  const startDensity = readFogVolumeComponent(id).density;
  let elapsedMs = 0;
  // Both tweens call readFogVolumeComponent independently, so if they fire
  // on the same tick, the second write clobbers the first axis's update.
  // Each tween converges correctly over subsequent ticks; one setComponent
  // call per tick is wasted when both axes are animating simultaneously.
  const ctrl = tickSubscription((ctx) => {
    elapsedMs += ctx.delta * 1000;
    const t = Math.min(1, elapsedMs / durationMs);
    const value = startDensity + (target - startDensity) * t;
    const live = readFogVolumeComponent(id);
    writeFogVolume(id, value, live.color as [number, number, number], live.scatter, live.edge_softness);
    if (t >= 1) {
      ctrl.stop();
      slots[DENSITY_TWEEN] = null;
    }
  });
  slots[DENSITY_TWEEN] = ctrl;
}

function startColorTween(
  id: EntityId,
  slots: TweenSlots,
  target: [number, number, number],
  durationMs: number,
): void {
  cancelExisting(slots, COLOR_TWEEN);
  const liveStart = readFogVolumeComponent(id);
  const from: [number, number, number] = [
    liveStart.color[0],
    liveStart.color[1],
    liveStart.color[2],
  ];
  let elapsedMs = 0;
  const ctrl = tickSubscription((ctx) => {
    elapsedMs += ctx.delta * 1000;
    const t = Math.min(1, elapsedMs / durationMs);
    const value: [number, number, number] = [
      from[0] + (target[0] - from[0]) * t,
      from[1] + (target[1] - from[1]) * t,
      from[2] + (target[2] - from[2]) * t,
    ];
    const live = readFogVolumeComponent(id);
    writeFogVolume(id, live.density, value, live.scatter, live.edge_softness);
    if (t >= 1) {
      ctrl.stop();
      slots[COLOR_TWEEN] = null;
    }
  });
  slots[COLOR_TWEEN] = ctrl;
}

/** Local alias matching the runtime shape that `setComponent` accepts for fog volumes. */
type ComponentValuePayload = {
  kind: "fog_volume";
  density: number;
  color: [number, number, number];
  scatter: number;
  edge_softness: number;
};

/**
 * Wrap a `FogVolumeEntity` snapshot returned by `worldQuery` as a
 * mutating handle. Used by `world.ts` for `world.query({ component:
 * "fog_volume" })`. The returned object preserves all snapshot fields
 * and adds the four mutating methods.
 */
export function wrapFogVolumeEntity(
  snapshot: GeneratedFogVolumeEntity,
): FogVolumeHandle {
  const id: EntityId = snapshot.id;
  const slots: TweenSlots = {};

  const handle: FogVolumeHandle = {
    ...snapshot,

    setDensity(density: number, durationMs: number = 0): void {
      if (durationMs <= 0) {
        cancelExisting(slots, DENSITY_TWEEN);
        const live = readFogVolumeComponent(id);
        writeFogVolume(id, density, live.color as [number, number, number], live.scatter, live.edge_softness);
        return;
      }
      startDensityTween(id, slots, density, durationMs);
    },

    setColor(
      color: [number, number, number],
      durationMs: number = 0,
    ): void {
      if (durationMs <= 0) {
        cancelExisting(slots, COLOR_TWEEN);
        const live = readFogVolumeComponent(id);
        writeFogVolume(id, live.density, color, live.scatter, live.edge_softness);
        return;
      }
      startColorTween(id, slots, color, durationMs);
    },

    setScatter(scatter: number): void {
      const live = readFogVolumeComponent(id);
      writeFogVolume(id, live.density, live.color as [number, number, number], scatter, live.edge_softness);
    },

    setEdgeSoftness(edgeSoftness: number): void {
      const live = readFogVolumeComponent(id);
      writeFogVolume(id, live.density, live.color as [number, number, number], live.scatter, edgeSoftness);
    },
  };

  return handle;
}

/**
 * Oscillates `handle.density` sinusoidally between `min` and `max`
 * with the given `period` (milliseconds). Returns a controller whose
 * `.stop()` cancels the animation. Multiple `pulseDensity` calls on
 * the same handle stack — call `.stop()` on the previous controller
 * before starting a new one if you do not want overlap.
 *
 * Note: cancelling via `.stop()` stops density updates but the tick
 * handler remains registered until the level unloads. Avoid calling
 * `pulseDensity` repeatedly on the same handle — prefer reusing the
 * controller returned from the first call.
 *
 * Implementation: a `tick` handler that writes a fresh density to the
 * fog_volume component each frame. No engine-side animation primitive
 * is required.
 */
export function pulseDensity(
  handle: FogVolumeHandle,
  opts: { min: number; max: number; period: number },
): AnimationController {
  const { min, max, period } = opts;
  if (!(period > 0)) {
    throw new Error("pulseDensity: `period` must be a positive number");
  }
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const mid = (lo + hi) * 0.5;
  const amp = (hi - lo) * 0.5;
  const id: EntityId = handle.id;
  let elapsedMs = 0;
  return tickSubscription((ctx) => {
    elapsedMs += ctx.delta * 1000;
    const phase = (elapsedMs % period) / period;
    const value = mid + amp * Math.sin(phase * Math.PI * 2);
    const live = readFogVolumeComponent(id);
    writeFogVolume(id, value, live.color as [number, number, number], live.scatter, live.edge_softness);
  });
}
