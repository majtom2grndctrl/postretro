// Passive world-presentation template descriptors. These are manifest data:
// constructing one never installs a UI tree, captures input, or performs GPU
// work. The renderer later owns layout of `root` for each stamped spawn.

import type { RuntimeValue } from "postretro";

import { runtime } from "../runtime";
import type { WidgetDescriptor, WidgetEasing } from "./widgets";

export type PresentationTemplateProps = {
  root: WidgetDescriptor;
  lifetimeMs: number;
  motion: { rise: number; easing: WidgetEasing };
  fade: { startMs: number };
  spawnScatter: { radius: number };
  /** Required by an overlay consumer; ignored by event-spawn presentation. */
  worldAnchor?: { socket: string; offsetY: number };
};

export type PresentationTemplate<Name extends string = string> = Readonly<{
  id: Name;
  root: WidgetDescriptor;
  lifetimeMs: number;
  motion: { rise: number; easing: WidgetEasing };
  fade: { startMs: number };
  spawnScatter: { radius: number };
  worldAnchor?: { socket: string; offsetY: number };
}>;

export type OverlayEntity = Readonly<{
  state(name: string): RuntimeValue;
}>;

export type DamagedEnemiesProps = {
  lingerMs: number;
  hideAtFull: boolean;
  shield?: {
    value: (entity: OverlayEntity) => RuntimeValue;
    max: (entity: OverlayEntity) => RuntimeValue;
  };
};

export type DamagedEnemiesSource = Readonly<{
  kind: "damagedEnemies";
  lingerMs: number;
  hideAtFull: boolean;
  shield?: { value: RuntimeValue; max: RuntimeValue };
}>;

export type PresentationOverlay = Readonly<{
  over: DamagedEnemiesSource;
  /** Stable id copied from the template; the template stays in presentationTemplates. */
  template: string;
  maxVisible: number;
}>;

const BINDING_NAME_SUGAR_DIAGNOSTIC =
  "definePresentationTemplate without an explicit id is binding-name sugar and must be used in a direct top-level binding declaration";

function requireFinite(value: unknown, field: string): asserts value is number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`definePresentationTemplate: \`${field}\` must be finite`);
  }
}

function validateProps(props: PresentationTemplateProps): void {
  if (props === null || typeof props !== "object" || Array.isArray(props)) {
    throw new TypeError("definePresentationTemplate: props must be an object");
  }
  if (props.root === null || typeof props.root !== "object" || typeof props.root.kind !== "string") {
    throw new TypeError("definePresentationTemplate: `root` must be a kind-tagged widget descriptor");
  }
  requireFinite(props.lifetimeMs, "lifetimeMs");
  if (props.lifetimeMs < 0 || !Number.isInteger(props.lifetimeMs)) {
    throw new TypeError("definePresentationTemplate: `lifetimeMs` must be a non-negative integer");
  }
  if (props.motion === null || typeof props.motion !== "object") {
    throw new TypeError("definePresentationTemplate: `motion` must be an object");
  }
  requireFinite(props.motion.rise, "motion.rise");
  if (!["linear", "easeIn", "easeOut", "easeInOut"].includes(props.motion.easing)) {
    throw new TypeError("definePresentationTemplate: `motion.easing` is invalid");
  }
  if (props.fade === null || typeof props.fade !== "object") {
    throw new TypeError("definePresentationTemplate: `fade` must be an object");
  }
  requireFinite(props.fade.startMs, "fade.startMs");
  if (props.fade.startMs < 0 || !Number.isInteger(props.fade.startMs) || props.fade.startMs > props.lifetimeMs) {
    throw new TypeError("definePresentationTemplate: `fade.startMs` must be an integer within the lifetime");
  }
  if (props.spawnScatter === null || typeof props.spawnScatter !== "object") {
    throw new TypeError("definePresentationTemplate: `spawnScatter` must be an object");
  }
  requireFinite(props.spawnScatter.radius, "spawnScatter.radius");
  if (props.spawnScatter.radius < 0) {
    throw new TypeError("definePresentationTemplate: `spawnScatter.radius` must be non-negative");
  }
  if (props.worldAnchor !== undefined) {
    if (props.worldAnchor === null || typeof props.worldAnchor !== "object") {
      throw new TypeError("definePresentationTemplate: `worldAnchor` must be an object");
    }
    if (typeof props.worldAnchor.socket !== "string" || props.worldAnchor.socket.length === 0) {
      throw new TypeError("definePresentationTemplate: `worldAnchor.socket` must be nonempty");
    }
    requireFinite(props.worldAnchor.offsetY, "worldAnchor.offsetY");
  }
}

/**
 * Declare a passive presentation template. In TypeScript the script compiler
 * inserts `id` from a direct `const template = ...` binding; authors supply no
 * mutable name field, which keeps manifest handles stable through refactors.
 */
export function definePresentationTemplate<const Props extends PresentationTemplateProps>(
  props: Props,
): PresentationTemplate<string>;
export function definePresentationTemplate(
  idOrProps: string | PresentationTemplateProps,
  maybeProps?: PresentationTemplateProps,
): PresentationTemplate {
  if (arguments.length === 1) throw new TypeError(BINDING_NAME_SUGAR_DIAGNOSTIC);
  const id = idOrProps as string;
  const props = maybeProps!;
  if (typeof id !== "string" || id.length === 0) {
    throw new TypeError("definePresentationTemplate: compiler-supplied id must be nonempty");
  }
  validateProps(props);
  return Object.freeze({
    id,
    root: props.root,
    lifetimeMs: props.lifetimeMs,
    motion: Object.freeze({ rise: props.motion.rise, easing: props.motion.easing }),
    fade: Object.freeze({ startMs: props.fade.startMs }),
    spawnScatter: Object.freeze({ radius: props.spawnScatter.radius }),
    ...(props.worldAnchor === undefined
      ? {}
      : { worldAnchor: Object.freeze({ socket: props.worldAnchor.socket, offsetY: props.worldAnchor.offsetY }) }),
  });
}

const OVERLAY_ENTITY: OverlayEntity = Object.freeze({
  state(name: string): RuntimeValue {
    if (typeof name !== "string" || name.length === 0) {
      throw new TypeError("damagedEnemies: state name must be nonempty");
    }
    return runtime.read(`@state.${name}`);
  },
});

/** Build the event-driven recently-damaged enemy source. The shield pair stays
 * un-divided so the host can derive presence and guard a zero denominator. */
export function damagedEnemies(props: DamagedEnemiesProps): DamagedEnemiesSource {
  if (props === null || typeof props !== "object" || Array.isArray(props)) {
    throw new TypeError("damagedEnemies: props must be an object");
  }
  requireFinite(props.lingerMs, "lingerMs");
  if (props.lingerMs < 0 || !Number.isInteger(props.lingerMs)) {
    throw new TypeError("damagedEnemies: `lingerMs` must be a non-negative integer");
  }
  if (typeof props.hideAtFull !== "boolean") {
    throw new TypeError("damagedEnemies: `hideAtFull` must be a boolean");
  }
  let shield: DamagedEnemiesSource["shield"];
  if (props.shield !== undefined) {
    if (props.shield === null || typeof props.shield !== "object"
      || typeof props.shield.value !== "function" || typeof props.shield.max !== "function") {
      throw new TypeError("damagedEnemies: `shield` must provide value and max expressions");
    }
    shield = Object.freeze({
      value: props.shield.value(OVERLAY_ENTITY),
      max: props.shield.max(OVERLAY_ENTITY),
    });
  }
  return Object.freeze({
    kind: "damagedEnemies",
    lingerMs: props.lingerMs,
    hideAtFull: props.hideAtFull,
    ...(shield === undefined ? {} : { shield }),
  });
}

/** Bind one passive template to a fact-driven source. Register the returned
 * descriptor in `defineMod({ presentationOverlays: [...] })`. */
export function defineOverlay(props: {
  over: DamagedEnemiesSource;
  template: PresentationTemplate;
  maxVisible: number;
}): PresentationOverlay {
  if (props === null || typeof props !== "object" || Array.isArray(props)) {
    throw new TypeError("defineOverlay: props must be an object");
  }
  if (props.over === null || typeof props.over !== "object" || props.over.kind !== "damagedEnemies") {
    throw new TypeError("defineOverlay: `over` must come from damagedEnemies");
  }
  if (props.template === null || typeof props.template !== "object" || typeof props.template.id !== "string") {
    throw new TypeError("defineOverlay: `template` must come from definePresentationTemplate");
  }
  if (props.template.worldAnchor === undefined) {
    throw new TypeError("defineOverlay: `template.worldAnchor` is required for an overlay");
  }
  requireFinite(props.maxVisible, "maxVisible");
  if (props.maxVisible < 1 || !Number.isInteger(props.maxVisible)) {
    throw new TypeError("defineOverlay: `maxVisible` must be a positive integer");
  }
  return Object.freeze({
    over: props.over,
    template: props.template.id,
    maxVisible: props.maxVisible,
  });
}
