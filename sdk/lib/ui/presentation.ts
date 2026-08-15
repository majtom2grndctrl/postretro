// Passive world-presentation template descriptors. These are manifest data:
// constructing one never installs a UI tree, captures input, or performs GPU
// work. The renderer later owns layout of `root` for each stamped spawn.

import type { WidgetDescriptor, WidgetEasing } from "./widgets";

export type PresentationTemplateProps = {
  root: WidgetDescriptor;
  lifetimeMs: number;
  motion: { rise: number; easing: WidgetEasing };
  fade: { startMs: number };
  spawnScatter: { radius: number };
};

export type PresentationTemplate<Name extends string = string> = Readonly<{
  id: Name;
  root: WidgetDescriptor;
  lifetimeMs: number;
  motion: { rise: number; easing: WidgetEasing };
  fade: { startMs: number };
  spawnScatter: { radius: number };
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
  });
}
