--!strict
-- UI theme authoring helpers. Mirrors `theme.ts`: returned theme data keeps the
-- flat runtime shape, with a `tokens` helper table for authoring-time lookup.

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

local function buildTokenAccessor(theme: any, field: string, category: string)
  return function(token: string): string
    if type(token) ~= "string" or #token == 0 then
      error(`defineTheme: {category} token must be a nonempty string`)
    end
    local tokens = theme[field]
    if type(tokens) ~= "table" or tokens[token] == nil then
      error(`defineTheme: unknown {category} token \`{token}\``)
    end
    return token
  end
end

function UiThemeSdk.defineTheme(theme: ThemeDefinition): DefinedTheme
  if type(theme) ~= "table" then
    error("defineTheme: theme must be a table")
  end
  if (theme :: any).tokens ~= nil then
    error("defineTheme: `tokens` is reserved")
  end
  local rawTheme = theme :: any
  rawTheme.tokens = {
    color = buildTokenAccessor(rawTheme, "colors", "color"),
    font = buildTokenAccessor(rawTheme, "fonts", "font"),
    spacing = buildTokenAccessor(rawTheme, "spacing", "spacing"),
  }
  return rawTheme
end

return UiThemeSdk
