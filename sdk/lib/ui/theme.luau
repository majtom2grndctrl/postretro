--!strict
-- UI theme authoring helpers. Authors declare nested singular token groups;
-- returned theme tables keep the flat manifest shape consumed by Rust.

export type ThemeColorValue = { number }
export type ThemeTokenTree<Leaf> = { [string]: Leaf | ThemeTokenTree<Leaf> }
export type ThemeDefinition = {
  color: ThemeTokenTree<ThemeColorValue>?,
  font: ThemeTokenTree<string>?,
  spacing: ThemeTokenTree<number>?,
}
export type TokenCategory = "color" | "font" | "spacing"
export type ColorToken = { __postretroToken: "color", token: string }
export type FontToken = { __postretroToken: "font", token: string }
export type SpacingToken = { __postretroToken: "spacing", token: string }
export type DesignTokenTree<Token> = Token & { [string]: DesignTokenTree<Token> }
export type DesignTokens = {
  color: DesignTokenTree<ColorToken>,
  font: DesignTokenTree<FontToken>,
  spacing: DesignTokenTree<SpacingToken>,
}
export type DefinedTheme = {
  colors: { [string]: ThemeColorValue },
  fonts: { [string]: string },
  spacing: { [string]: number },
}

local UiThemeSdk = {}
local designTokensByTheme = setmetatable({}, { __mode = "k" })
local tokenCategories = setmetatable({}, { __mode = "k" })
local SPECIAL_THEME_KEYS = {
  ["__proto__"] = true,
  prototype = true,
  constructor = true,
}

local function isFiniteNumber(value: any): boolean
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end

local function pathToString(pathParts: { string }): string
  return table.concat(pathParts, ".")
end

local function validateKey(key: any, category: string, path: string): string
  if
    type(key) ~= "string"
    or #key == 0
    or string.find(key, "%.", 1, false) ~= nil
    or SPECIAL_THEME_KEYS[key] == true
  then
    local location = if #path == 0 then category else `{category}.{path}`
    error(`defineTheme: invalid key under {location}`)
  end
  return key
end

local function colorLeaf(value: any, path: string): ThemeColorValue?
  if type(value) ~= "table" then
    return nil
  end

  local hasNumberKey = false
  local hasStringKey = false
  for key, _ in pairs(value) do
    if type(key) == "number" then
      hasNumberKey = true
    elseif type(key) == "string" then
      hasStringKey = true
    else
      error(`defineTheme: color.{path} has an invalid key`)
    end
  end

  if not hasNumberKey then
    return nil
  end
  if hasStringKey then
    error(`defineTheme: color.{path} mixes a color leaf with child groups`)
  end

  for index = 1, 4 do
    if not isFiniteNumber(value[index]) then
      error(`defineTheme: color.{path} must be exactly four finite numbers`)
    end
  end
  for key, _ in pairs(value) do
    if type(key) ~= "number" or key < 1 or key > 4 or key % 1 ~= 0 then
      error(`defineTheme: color.{path} must be exactly four finite numbers`)
    end
  end

  return { value[1], value[2], value[3], value[4] }
end

local function fontLeaf(value: any, path: string): string?
  if type(value) ~= "string" then
    return nil
  end
  if #value == 0 then
    error(`defineTheme: font.{path} must be a non-empty string`)
  end
  return value
end

local function spacingLeaf(value: any, path: string): number?
  if type(value) ~= "number" then
    return nil
  end
  if not isFiniteNumber(value) then
    error(`defineTheme: spacing.{path} must be a finite number`)
  end
  return value
end

local function tokenLeaf(category: TokenCategory, token: string)
  local leaf = table.freeze({
    __postretroToken = category,
    token = token,
  })
  tokenCategories[leaf] = category
  return leaf
end

local function finalizeTokenTree(category: TokenCategory, path: string, target: { [string]: any })
  for key, value in pairs(target) do
    if type(value) == "table" and tokenCategories[value] == nil then
      target[key] = finalizeTokenTree(category, if #path == 0 then key else `{path}.{key}`, value)
    end
  end
  setmetatable(target, {
    __index = function(_, key)
      if type(key) == "string" then
        local fullPath = if #path == 0 then key else `{path}.{key}`
        error(`getDesignTokens: unknown {category} token \`{fullPath}\``)
      end
      return nil
    end,
    __newindex = function()
      error("getDesignTokens: token trees are read-only")
    end,
  })
  return table.freeze(target)
end

function UiThemeSdk.__unwrapThemeToken(value: any, category: TokenCategory): string?
  if type(value) ~= "table" then
    return nil
  end
  if tokenCategories[value] ~= category then
    return nil
  end
  return (value :: any).token
end

local function flattenThemeGroup<Leaf>(
  group: any,
  category: TokenCategory,
  readLeaf: (any, string) -> Leaf?
): ({ [string]: Leaf }, { [string]: any })
  if group == nil then
    return {}, finalizeTokenTree(category, "", {})
  end
  if type(group) ~= "table" then
    error(`defineTheme: {category} must be a table`)
  end

  local flat = {}
  local tokens = {}
  local seen = {}

  local function visit(node: any, pathParts: { string }, out: { [string]: any })
    local path = pathToString(pathParts)
    local leaf = if #path > 0 then readLeaf(node, path) else nil
    if leaf ~= nil then
      if seen[path] then
        error(`defineTheme: duplicate {category} token \`{path}\``)
      end
      seen[path] = true
      flat[path] = leaf
      return
    end

    if type(node) ~= "table" then
      local location = if #path == 0 then category else `{category}.{path}`
      error(`defineTheme: malformed {location} token leaf`)
    end

    for rawKey, childValue in pairs(node) do
      local key = validateKey(rawKey, category, path)
      local childPathParts = {}
      for index, part in ipairs(pathParts) do
        childPathParts[index] = part
      end
      table.insert(childPathParts, key)
      local childPath = pathToString(childPathParts)
      local childLeaf = readLeaf(childValue, childPath)
      if childLeaf ~= nil then
        if seen[childPath] then
          error(`defineTheme: duplicate {category} token \`{childPath}\``)
        end
        seen[childPath] = true
        flat[childPath] = childLeaf
        out[key] = tokenLeaf(category, childPath)
      else
        local childTokens = {}
        visit(childValue, childPathParts, childTokens)
        out[key] = childTokens
      end
    end
  end

  visit(group, {}, tokens)
  return flat, finalizeTokenTree(category, "", tokens)
end

function UiThemeSdk.defineTheme(theme: ThemeDefinition): DefinedTheme
  if type(theme) ~= "table" then
    error("defineTheme: theme must be a table")
  end
  local rawTheme = theme :: any
  if rawget(rawTheme, "colors") ~= nil then
    error("defineTheme: `colors` is unsupported")
  end
  if rawget(rawTheme, "fonts") ~= nil then
    error("defineTheme: `fonts` is unsupported")
  end
  if rawget(rawTheme, "tokens") ~= nil then
    error("defineTheme: `tokens` is unsupported")
  end

  local colors, colorTokens = flattenThemeGroup(rawTheme.color, "color", colorLeaf)
  local fonts, fontTokens = flattenThemeGroup(rawTheme.font, "font", fontLeaf)
  local spacing, spacingTokens = flattenThemeGroup(rawTheme.spacing, "spacing", spacingLeaf)

  local defined = {
    colors = colors,
    fonts = fonts,
    spacing = spacing,
  }
  designTokensByTheme[defined] = {
    color = colorTokens,
    font = fontTokens,
    spacing = spacingTokens,
  }

  return defined
end

function UiThemeSdk.getDesignTokens(theme: DefinedTheme): DesignTokens
  if type(theme) ~= "table" then
    error("getDesignTokens: theme must be returned by defineTheme")
  end
  local tokens = designTokensByTheme[theme :: any]
  if tokens == nil then
    error("getDesignTokens: theme must be returned by defineTheme")
  end
  return tokens
end

return UiThemeSdk
