// UI theme authoring helpers.
//
// Authors declare nested singular token groups (`color`, `font`, `spacing`).
// The returned value keeps the runtime manifest wire shape: enumerable flat
// `colors`, `fonts`, and `spacing` maps. The nested token tree is SDK-only
// metadata for `getDesignTokens`.

export type ThemeColorValue = readonly [number, number, number, number];

declare const themeTokenBrand: unique symbol;

export type ThemeToken<Category extends "color" | "font" | "spacing"> = Readonly<{
  __postretroToken: Category;
  token: string;
  readonly [themeTokenBrand]: Category;
}>;
export type ColorToken = ThemeToken<"color">;
export type FontToken = ThemeToken<"font">;
export type SpacingToken = ThemeToken<"spacing">;

export type ThemeTokenTree<Leaf> = {
  readonly [key: string]: Leaf | ThemeTokenTree<Leaf>;
};

export type ThemeDefinition = {
  readonly color?: ThemeTokenTree<ThemeColorValue>;
  readonly font?: ThemeTokenTree<string>;
  readonly spacing?: ThemeTokenTree<number>;
  readonly colors?: never;
  readonly fonts?: never;
  readonly tokens?: never;
};

type JoinPath<Prefix extends string, Key extends string> = Prefix extends ""
  ? Key
  : `${Prefix}.${Key}`;

type FlattenTokenKeys<Tree, Leaf, Prefix extends string = ""> = Tree extends Leaf
  ? Prefix
  : Tree extends Readonly<Record<string, unknown>>
    ? {
        [K in Extract<keyof Tree, string>]: FlattenTokenKeys<
          Tree[K],
          Leaf,
          JoinPath<Prefix, K>
        >;
      }[Extract<keyof Tree, string>]
    : never;

type FlatTokenMap<Tree, Leaf, Value> = Record<
  FlattenTokenKeys<NonNullable<Tree>, Leaf>,
  Value
>;

type DesignTokenTree<Tree, Leaf, Token, Prefix extends string = ""> = Tree extends Leaf
  ? Token
  : Tree extends Readonly<Record<string, unknown>>
    ? {
        readonly [K in Extract<keyof Tree, string>]: DesignTokenTree<
          Tree[K],
          Leaf,
          Token,
          JoinPath<Prefix, K>
        >;
      }
    : never;

type DesignTokenGroup<Tree, Leaf, Token> = [Tree] extends [undefined]
  ? {}
  : DesignTokenTree<NonNullable<Tree>, Leaf, Token>;

export type DesignTokens<T extends ThemeDefinition> = {
  readonly color: DesignTokenGroup<T["color"], ThemeColorValue, ColorToken>;
  readonly font: DesignTokenGroup<T["font"], string, FontToken>;
  readonly spacing: DesignTokenGroup<T["spacing"], number, SpacingToken>;
};

declare const definedThemeBrand: unique symbol;

export type DefinedTheme<T extends ThemeDefinition> = {
  readonly colors: FlatTokenMap<T["color"], ThemeColorValue, ThemeColorValue>;
  readonly fonts: FlatTokenMap<T["font"], string, string>;
  readonly spacing: FlatTokenMap<T["spacing"], number, number>;
  readonly [definedThemeBrand]: T;
};

const designTokensByTheme = new WeakMap<object, DesignTokens<ThemeDefinition>>();
const tokenMetadataByLeaf = new WeakMap<object, { category: "color" | "font" | "spacing"; token: string }>();
const designTokensMetadata = Symbol("postretroDesignTokens");
const SPECIAL_THEME_KEYS = new Set(["__proto__", "prototype", "constructor"]);

function assertDefinitionObject(value: unknown): asserts value is Record<PropertyKey, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("defineTheme: theme must be an object");
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new Error("defineTheme: theme must be a plain object");
  }
}

function assertNoUnsupportedFields(theme: Record<PropertyKey, unknown>): void {
  for (const key of ["colors", "fonts", "tokens"] as const) {
    if (Object.prototype.hasOwnProperty.call(theme, key)) {
      throw new Error(`defineTheme: \`${key}\` is unsupported`);
    }
  }
}

function assertValidKey(key: PropertyKey, category: string, path: string): asserts key is string {
  if (
    typeof key !== "string" ||
    key.length === 0 ||
    key.includes(".") ||
    SPECIAL_THEME_KEYS.has(key)
  ) {
    const location = path.length === 0 ? category : `${category}.${path}`;
    throw new Error(`defineTheme: invalid key under ${location}`);
  }
}

function assertThemeGroupObject(
  value: unknown,
  category: "color" | "font" | "spacing",
  path: string,
): asserts value is Record<PropertyKey, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    const location = path.length === 0 ? category : `${category}.${path}`;
    throw new Error(`defineTheme: malformed ${location} token leaf`);
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    const location = path.length === 0 ? category : `${category}.${path}`;
    throw new Error(`defineTheme: ${location} must be a plain object`);
  }
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function validateColorLeaf(value: unknown, path: string): ThemeColorValue | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const keys = Object.keys(value);
  const hasOnlyTupleKeys =
    value.length === 4 &&
    keys.length === 4 &&
    keys.every((key) => key === "0" || key === "1" || key === "2" || key === "3");
  if (!hasOnlyTupleKeys || !value.every(isFiniteNumber)) {
    throw new Error(`defineTheme: color.${path} must be exactly four finite numbers`);
  }
  return [value[0], value[1], value[2], value[3]];
}

function validateFontLeaf(value: unknown, path: string): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  if (value.length === 0) {
    throw new Error(`defineTheme: font.${path} must be a non-empty string`);
  }
  return value;
}

function validateSpacingLeaf(value: unknown, path: string): number | undefined {
  if (typeof value !== "number") {
    return undefined;
  }
  if (!Number.isFinite(value)) {
    throw new Error(`defineTheme: spacing.${path} must be a finite number`);
  }
  return value;
}

function tokenLeaf<Category extends "color" | "font" | "spacing">(
  category: Category,
  token: string,
): ThemeToken<Category> {
  const leaf = Object.freeze({
    __postretroToken: category,
    token,
  }) as ThemeToken<Category>;
  tokenMetadataByLeaf.set(leaf, { category, token });
  return leaf;
}

export function __unwrapThemeToken(
  value: unknown,
  category: "color" | "font" | "spacing",
): string | undefined {
  if (value === null || typeof value !== "object") {
    return undefined;
  }
  const metadata = tokenMetadataByLeaf.get(value);
  return metadata?.category === category ? metadata.token : undefined;
}

function missingTokenError(category: "color" | "font" | "spacing", path: string, key: string): Error {
  const fullPath = path.length === 0 ? key : `${path}.${key}`;
  return new Error(`getDesignTokens: unknown ${category} token \`${fullPath}\``);
}

function finalizeTokenTree<Category extends "color" | "font" | "spacing">(
  target: Record<string, unknown>,
  category: Category,
  path: string,
): Record<string, unknown> {
  for (const key of Object.keys(target)) {
    const value = target[key];
    if (value !== null && typeof value === "object" && tokenMetadataByLeaf.get(value) === undefined) {
      target[key] = finalizeTokenTree(value as Record<string, unknown>, category, path.length === 0 ? key : `${path}.${key}`);
    }
  }
  Object.freeze(target);
  return new Proxy(target, {
    get(obj, prop, receiver) {
      if (typeof prop === "symbol") {
        return Reflect.get(obj, prop, receiver);
      }
      if (Object.prototype.hasOwnProperty.call(obj, prop)) {
        return Reflect.get(obj, prop, receiver);
      }
      throw missingTokenError(category, path, prop);
    },
    set() {
      throw new Error("getDesignTokens: token trees are read-only");
    },
    defineProperty() {
      throw new Error("getDesignTokens: token trees are read-only");
    },
    deleteProperty() {
      throw new Error("getDesignTokens: token trees are read-only");
    },
  });
}

function flattenThemeGroup<Leaf, Category extends "color" | "font" | "spacing">(
  group: unknown,
  category: Category,
  readLeaf: (value: unknown, path: string) => Leaf | undefined,
): { flat: Record<string, Leaf>; tokens: Record<string, unknown> } {
  if (group === undefined) {
    return { flat: Object.create(null) as Record<string, Leaf>, tokens: finalizeTokenTree(Object.create(null), category, "") };
  }
  if (typeof group !== "object" || group === null || Array.isArray(group)) {
    throw new Error(`defineTheme: ${category} must be an object`);
  }
  const prototype = Object.getPrototypeOf(group);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new Error(`defineTheme: ${category} must be a plain object`);
  }

  const flat: Record<string, Leaf> = Object.create(null) as Record<string, Leaf>;
  const tokens: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
  const seen = new Set<string>();

  function visit(node: unknown, pathParts: string[], out: Record<string, unknown>): void {
    const path = pathParts.join(".");
    const leaf = path.length === 0 ? undefined : readLeaf(node, path);
    if (leaf !== undefined) {
      if (seen.has(path)) {
        throw new Error(`defineTheme: duplicate ${category} token \`${path}\``);
      }
      seen.add(path);
      flat[path] = leaf;
      return;
    }

    assertThemeGroupObject(node, category, path);

    for (const key of Reflect.ownKeys(node)) {
      assertValidKey(key, category, path);
      const childPathParts = [...pathParts, key];
      const childPath = childPathParts.join(".");
      const childValue = (node as Record<string, unknown>)[key];
      const childLeaf = readLeaf(childValue, childPath);
      if (childLeaf !== undefined) {
        if (seen.has(childPath)) {
          throw new Error(`defineTheme: duplicate ${category} token \`${childPath}\``);
        }
        seen.add(childPath);
        flat[childPath] = childLeaf;
        out[key] = tokenLeaf(category, childPath);
      } else {
        const childTokens: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
        visit(childValue, childPathParts, childTokens);
        out[key] = childTokens;
      }
    }
  }

  visit(group, [], tokens);
  return { flat, tokens: finalizeTokenTree(tokens, category, "") };
}

export function defineTheme<const T extends ThemeDefinition>(theme: T): DefinedTheme<T> {
  assertDefinitionObject(theme);
  assertNoUnsupportedFields(theme);

  const color = flattenThemeGroup(theme.color, "color", validateColorLeaf);
  const font = flattenThemeGroup(theme.font, "font", validateFontLeaf);
  const spacing = flattenThemeGroup(theme.spacing, "spacing", validateSpacingLeaf);

  const defined = {
    colors: color.flat,
    fonts: font.flat,
    spacing: spacing.flat,
  } as DefinedTheme<T>;

  const designTokens = {
    color: color.tokens,
    font: font.tokens,
    spacing: spacing.tokens,
  } as DesignTokens<T>;

  Object.defineProperty(defined, designTokensMetadata, {
    value: designTokens,
    enumerable: false,
    configurable: false,
    writable: false,
  });
  designTokensByTheme.set(defined, designTokens as DesignTokens<ThemeDefinition>);

  return defined;
}

export function getDesignTokens<const T extends ThemeDefinition>(
  theme: DefinedTheme<T>,
): DesignTokens<T> {
  if (typeof theme !== "object" || theme === null) {
    throw new Error("getDesignTokens: theme must be returned by defineTheme");
  }
  const tokens = designTokensByTheme.get(theme);
  if (tokens === undefined) {
    throw new Error("getDesignTokens: theme must be returned by defineTheme");
  }
  return tokens as DesignTokens<T>;
}
