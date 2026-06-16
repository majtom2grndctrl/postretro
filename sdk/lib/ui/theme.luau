--!strict
-- UI theme authoring helpers. Mirrors `theme.ts`: returned theme data keeps the
-- flat runtime shape, with a non-enumerable `tokens` helper for authoring-time
-- lookup.

export type ThemeColorValue = { number }
export type ThemeDefinition = {
  colors: { [string]: ThemeColorValue }?,
  fonts: { [string]: string }?,
  spacing: { [string]: number }?,
}
export type ThemeTokenAccessors = {
  color: (string) -> string,
  font: (string) -> string,
  spacing: (string) -> string,
}
export type DefinedTheme = ThemeDefinition & {
  tokens: ThemeTokenAccessors,
}

local UiThemeSdk = {}

local function warnInvalidToken(category: string, token: string)
  local message
  if #token == 0 then
    message = `defineTheme: empty {category} token will degrade at UI resolution`
  else
    message = `defineTheme: unknown {category} token \`{token}\` will degrade at UI resolution`
  end
  if type(warn) == "function" then
    (warn :: any)(message)
  elseif type(print) == "function" then
    print(message)
  end
end

local function snapshotTokenNames(tokens: any): { [string]: boolean }
  local names = {}
  if type(tokens) == "table" then
    for key, _ in pairs(tokens) do
      if type(key) == "string" then
        names[key] = true
      end
    end
  end
  return names
end

local function buildTokenAccessor(tokens: any, category: string)
  local names = snapshotTokenNames(tokens)
  return function(token: any): string
    local name = if type(token) == "string" then token else tostring(token)
    if type(token) ~= "string" or #name == 0 or not names[name] then
      warnInvalidToken(category, name)
    end
    return name
  end
end

function UiThemeSdk.defineTheme(theme: ThemeDefinition): DefinedTheme
  if type(theme) ~= "table" then
    error("defineTheme: theme must be a table")
  end
  if rawget(theme :: any, "tokens") ~= nil then
    error("defineTheme: `tokens` is reserved")
  end
  local rawTheme = theme :: any
  local tokenAccessors = {
    color = buildTokenAccessor(rawTheme.colors, "color"),
    font = buildTokenAccessor(rawTheme.fonts, "font"),
    spacing = buildTokenAccessor(rawTheme.spacing, "spacing"),
  }
  setmetatable(rawTheme, {
    __index = function(_, key)
      if key == "tokens" then
        return tokenAccessors
      end
      return nil
    end,
  })
  return rawTheme
end

return UiThemeSdk
