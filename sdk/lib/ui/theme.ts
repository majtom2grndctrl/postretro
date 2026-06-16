// UI theme authoring helpers.
//
// Theme data remains the runtime wire shape: flat token maps for colors, fonts,
// and spacing. `defineTheme` adds non-enumerable token accessors so TypeScript
// editors can autocomplete keys from the concrete theme object while descriptor
// fields still receive plain token strings.

export type ThemeColorValue = readonly [number, number, number, number];

export type ThemeDefinition = {
  readonly colors?: Readonly<Record<string, ThemeColorValue>>;
  readonly fonts?: Readonly<Record<string, string>>;
  readonly spacing?: Readonly<Record<string, number>>;
};

type ThemeTokenMap<T extends ThemeDefinition, K extends keyof ThemeDefinition> = Extract<
  NonNullable<T[K]>,
  Readonly<Record<string, unknown>>
>;
type ThemeTokenKeys<T extends ThemeDefinition, K extends keyof ThemeDefinition> = [
  ThemeTokenMap<T, K>,
] extends [never]
  ? never
  : Extract<keyof ThemeTokenMap<T, K>, string>;

export type ThemeTokenAccessors<T extends ThemeDefinition> = {
  color<K extends ThemeTokenKeys<T, "colors">>(token: K): K;
  font<K extends ThemeTokenKeys<T, "fonts">>(token: K): K;
  spacing<K extends ThemeTokenKeys<T, "spacing">>(token: K): K;
};

export type DefinedTheme<T extends ThemeDefinition> = T & {
  readonly tokens: ThemeTokenAccessors<T>;
};

function buildTokenAccessor(
  tokens: Readonly<Record<string, unknown>> | undefined,
  category: "color" | "font" | "spacing",
): (token: string) => string {
  return (token: string) => {
    if (tokens === undefined || !Object.prototype.hasOwnProperty.call(tokens, token)) {
      throw new Error(`defineTheme: unknown ${category} token \`${token}\``);
    }
    return token;
  };
}

export function defineTheme<const T extends ThemeDefinition>(theme: T): DefinedTheme<T> {
  if (Object.prototype.hasOwnProperty.call(theme, "tokens")) {
    throw new Error("defineTheme: `tokens` is reserved");
  }

  const tokens = {
    color: buildTokenAccessor(theme.colors, "color"),
    font: buildTokenAccessor(theme.fonts, "font"),
    spacing: buildTokenAccessor(theme.spacing, "spacing"),
  } as ThemeTokenAccessors<T>;

  Object.defineProperty(theme, "tokens", {
    value: tokens,
    enumerable: false,
    configurable: false,
    writable: false,
  });

  return theme as DefinedTheme<T>;
}
