// Passive world-presentation template descriptors. These are manifest data:
// constructing one never installs a UI tree, captures input, or performs GPU
// work. The renderer later owns layout of `root` for each stamped spawn.

import type { RuntimeValue } from "postretro";

import { runtime } from "../runtime";
import type { FactBindRef, NumberTween, WidgetDescriptor, WidgetEasing } from "./widgets";

export type NumberFactOptions = { format?: string; tween?: NumberTween };
export type ScalarFactOptions = { format?: string };

export type PresentationFactApi = Readonly<{
  number(name: string, options?: NumberFactOptions): FactBindRef<number> & NumberFactOptions;
  text(name: string, options?: ScalarFactOptions): FactBindRef<string> & ScalarFactOptions;
  bool(name: string, options?: ScalarFactOptions): FactBindRef<boolean> & ScalarFactOptions;
}>;

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
const MAX_OVERLAY_VISIBLE = 64;
const MAX_U32 = 4_294_967_295;
const MAX_F32 = 3.4028234663852886e38;

function requireStoredF32(value: unknown, field: string, factory: string): asserts value is number {
  if (typeof value !== "number" || !Number.isFinite(value) || Math.abs(value) > MAX_F32) {
    throw new TypeError(`${factory}: \`${field}\` must be a finite number representable as f32`);
  }
}

function requireStoredU32(value: unknown, field: string, factory: string): asserts value is number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > MAX_U32) {
    throw new TypeError(`${factory}: \`${field}\` must be an integer in [0, ${MAX_U32}]`);
  }
}

function requireFactName(name: unknown): asserts name is string {
  if (typeof name !== "string" || name.length === 0) {
    throw new TypeError("fact: name must be nonempty");
  }
}

function requireFiniteFact(value: unknown, field: string): asserts value is number {
  requireStoredF32(value, field, "fact");
}

function factRef(
  name: string,
  options: NumberFactOptions | ScalarFactOptions = {},
): Readonly<{ fact: string; format?: string; tween?: NumberTween }> {
  requireFactName(name);
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("fact: options must be an object");
  }
  if (options.format !== undefined && typeof options.format !== "string") {
    throw new TypeError("fact: `format` must be a string");
  }
  const tween = (options as NumberFactOptions).tween;
  if (tween !== undefined) {
    if (tween === null || typeof tween !== "object") {
      throw new TypeError("fact: `tween` must be an object");
    }
    requireFiniteFact(tween.durationMs, "tween.durationMs");
    if (tween.durationMs < 0) {
      throw new TypeError("fact: `tween.durationMs` must be non-negative");
    }
    if (!["linear", "easeIn", "easeOut", "easeInOut"].includes(tween.easing)) {
      throw new TypeError("fact: `tween.easing` is invalid");
    }
    if (tween.from !== undefined) requireFiniteFact(tween.from, "tween.from");
  }
  return Object.freeze({
    fact: name,
    ...(options.format === undefined ? {} : { format: options.format }),
    ...(tween === undefined
      ? {}
      : {
          tween: Object.freeze({
            durationMs: tween.durationMs,
            easing: tween.easing,
            ...(tween.from === undefined ? {} : { from: tween.from }),
          }),
        }),
  });
}

/** Bind producer-stamped per-instance values inside a presentation template. */
export const fact: PresentationFactApi = Object.freeze({
  number(name: string, options?: NumberFactOptions) {
    return factRef(name, options) as FactBindRef<number> & NumberFactOptions;
  },
  text(name: string, options?: ScalarFactOptions) {
    return factRef(name, options) as FactBindRef<string> & ScalarFactOptions;
  },
  bool(name: string, options?: ScalarFactOptions) {
    return factRef(name, options) as FactBindRef<boolean> & ScalarFactOptions;
  },
});

function validateWidgetF32(value: unknown, field: string, positive = false): void {
  requireStoredF32(value, field, "definePresentationTemplate");
  if ((positive && value <= 0) || (!positive && value < 0)) {
    throw new TypeError(
      `definePresentationTemplate: \`${field}\` must be ${positive ? "greater than zero" : "non-negative"}`,
    );
  }
}

function validateColor(value: unknown, field: string): void {
  if (!Array.isArray(value)) return;
  if (value.length !== 4) {
    throw new TypeError(`definePresentationTemplate: \`${field}\` must contain four f32 components`);
  }
  value.forEach((component, index) =>
    requireStoredF32(component, `${field}[${index}]`, "definePresentationTemplate"));
}

function validateBorder(value: unknown, field: string): void {
  if (value === undefined) return;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`definePresentationTemplate: \`${field}\` must be an object`);
  }
  const border = value as Record<string, unknown>;
  if (typeof border.texture !== "string" || border.texture.length === 0) {
    throw new TypeError(`definePresentationTemplate: \`${field}.texture\` must be nonempty`);
  }
  if (!Array.isArray(border.slice) || border.slice.length !== 4) {
    throw new TypeError(`definePresentationTemplate: \`${field}.slice\` must contain four f32 dimensions`);
  }
  border.slice.forEach((dimension, index) =>
    validateWidgetF32(dimension, `${field}.slice[${index}]`));
  validateColor(border.tint, `${field}.tint`);
}

function validateTween(value: unknown, field: string): void {
  if (value === undefined) return;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`definePresentationTemplate: \`${field}\` must be an object`);
  }
  const tween = value as Record<string, unknown>;
  validateWidgetF32(tween.durationMs, `${field}.durationMs`);
  if (tween.from !== undefined) requireStoredF32(tween.from, `${field}.from`, "definePresentationTemplate");
}

function validateStyleRanges(value: unknown, field: string): void {
  if (value === undefined) return;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`definePresentationTemplate: \`${field}\` must be an object`);
  }
  const ranges = value as Record<string, unknown>;
  validateWidgetF32(ranges.max, `${field}.max`, true);
  if (!Array.isArray(ranges.entries)) {
    throw new TypeError(`definePresentationTemplate: \`${field}.entries\` must be an array`);
  }
  ranges.entries.forEach((entry, index) => {
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) {
      throw new TypeError(`definePresentationTemplate: \`${field}.entries[${index}]\` must be an object`);
    }
    const item = entry as Record<string, unknown>;
    if (item.upTo !== undefined) requireStoredF32(item.upTo, `${field}.entries[${index}].upTo`, "definePresentationTemplate");
    if (item.color !== undefined) validateColor(item.color, `${field}.entries[${index}].color`);
    if (item.pulse !== undefined) {
      const pulse = item.pulse as Record<string, unknown>;
      validateWidgetF32(pulse?.periodMs, `${field}.entries[${index}].pulse.periodMs`, true);
    }
    if (item.flash !== undefined) {
      const flash = item.flash as Record<string, unknown>;
      validateWidgetF32(flash?.durationMs, `${field}.entries[${index}].flash.durationMs`);
    }
  });
}

function validatePassiveWidget(value: unknown, path: string): void {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`definePresentationTemplate: \`${path}\` must be a kind-tagged widget descriptor`);
  }
  const widget = value as Record<string, unknown>;
  if (typeof widget.kind !== "string") {
    throw new TypeError(`definePresentationTemplate: \`${path}.kind\` must be a string`);
  }
  if (!["text", "bar", "image", "vstack", "hstack"].includes(widget.kind)) {
    throw new TypeError(
      `definePresentationTemplate: \`${path}.kind\` "${widget.kind}" is not supported; passive templates allow text, bar, image, vstack, and hstack`,
    );
  }

  if (widget.kind === "text") {
    validateWidgetF32(widget.fontSize, `${path}.fontSize`, true);
    validateColor(widget.color, `${path}.color`);
  } else if (widget.kind === "bar") {
    if (typeof widget.max === "number") validateWidgetF32(widget.max, `${path}.max`, true);
    if (widget.width !== undefined) validateWidgetF32(widget.width, `${path}.width`, true);
    if (widget.height !== undefined) validateWidgetF32(widget.height, `${path}.height`, true);
    validateColor(widget.fill, `${path}.fill`);
    validateColor(widget.background, `${path}.background`);
    if (widget.exitFade !== undefined) {
      if (widget.exitFade === null || typeof widget.exitFade !== "object" || Array.isArray(widget.exitFade)) {
        throw new TypeError(`definePresentationTemplate: \`${path}.exitFade\` must be an object`);
      }
      validateWidgetF32(
        (widget.exitFade as Record<string, unknown>).durationMs,
        `${path}.exitFade.durationMs`,
        true,
      );
    }
  } else if (widget.kind === "vstack" || widget.kind === "hstack") {
    if (typeof widget.gap === "number") validateWidgetF32(widget.gap, `${path}.gap`);
    if (typeof widget.padding === "number") validateWidgetF32(widget.padding, `${path}.padding`);
    if (widget.fill !== undefined) validateColor(widget.fill, `${path}.fill`);
    validateBorder(widget.border, `${path}.border`);
    if (!Array.isArray(widget.children)) {
      throw new TypeError(`definePresentationTemplate: \`${path}.children\` must be an array`);
    }
    widget.children.forEach((child, index) => validatePassiveWidget(child, `${path}.children[${index}]`));
  }

  const bind = widget.bind as Record<string, unknown> | undefined;
  if (bind !== undefined) validateTween(bind.tween, `${path}.bind.tween`);
  validateStyleRanges(widget.styleRanges, `${path}.styleRanges`);
}

function validateProps(props: PresentationTemplateProps): void {
  if (props === null || typeof props !== "object" || Array.isArray(props)) {
    throw new TypeError("definePresentationTemplate: props must be an object");
  }
  if (props.root === null || typeof props.root !== "object" || typeof props.root.kind !== "string") {
    throw new TypeError("definePresentationTemplate: `root` must be a kind-tagged widget descriptor");
  }
  validatePassiveWidget(props.root, "root");
  requireStoredU32(props.lifetimeMs, "lifetimeMs", "definePresentationTemplate");
  if (props.motion === null || typeof props.motion !== "object") {
    throw new TypeError("definePresentationTemplate: `motion` must be an object");
  }
  requireStoredF32(props.motion.rise, "motion.rise", "definePresentationTemplate");
  if (!["linear", "easeIn", "easeOut", "easeInOut"].includes(props.motion.easing)) {
    throw new TypeError("definePresentationTemplate: `motion.easing` is invalid");
  }
  if (props.fade === null || typeof props.fade !== "object") {
    throw new TypeError("definePresentationTemplate: `fade` must be an object");
  }
  requireStoredU32(props.fade.startMs, "fade.startMs", "definePresentationTemplate");
  if (props.fade.startMs > props.lifetimeMs) {
    throw new TypeError("definePresentationTemplate: `fade.startMs` must not exceed `lifetimeMs`");
  }
  if (props.spawnScatter === null || typeof props.spawnScatter !== "object") {
    throw new TypeError("definePresentationTemplate: `spawnScatter` must be an object");
  }
  validateWidgetF32(props.spawnScatter.radius, "spawnScatter.radius");
  if (props.worldAnchor !== undefined) {
    if (props.worldAnchor === null || typeof props.worldAnchor !== "object") {
      throw new TypeError("definePresentationTemplate: `worldAnchor` must be an object");
    }
    if (typeof props.worldAnchor.socket !== "string" || props.worldAnchor.socket.length === 0) {
      throw new TypeError("definePresentationTemplate: `worldAnchor.socket` must be nonempty");
    }
    requireStoredF32(props.worldAnchor.offsetY, "worldAnchor.offsetY", "definePresentationTemplate");
  }
}

/**
 * Declare a passive presentation template. In TypeScript the script compiler
 * inserts `id` from a direct `const template = ...` binding. The binding
 * identifier is the id, so renaming the binding changes the manifest handle.
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
  requireStoredU32(props.lingerMs, "lingerMs", "damagedEnemies");
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
 * descriptor directly as `defineMod({ presentationOverlays: overlay })`. */
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
  if (typeof props.maxVisible !== "number" || props.maxVisible < 1 || props.maxVisible > MAX_OVERLAY_VISIBLE || !Number.isInteger(props.maxVisible)) {
    throw new TypeError(`defineOverlay: \`maxVisible\` must be an integer between 1 and ${MAX_OVERLAY_VISIBLE}`);
  }
  return Object.freeze({
    over: props.over,
    template: props.template.id,
    maxVisible: props.maxVisible,
  });
}
