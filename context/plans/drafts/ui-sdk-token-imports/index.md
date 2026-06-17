# UI SDK Token Imports

> **Status:** draft
> **Depends on:** `context/plans/drafts/luau-sdk-virtual-modules/`
> **Related:** `context/lib/ui.md` · `context/lib/scripting.md` · `sdk/lib/ui/theme.ts` · `sdk/lib/ui/theme.luau`

## Goal

Replace flat string-token UI authoring with nested theme objects and typed token
leaves. Move UI authoring APIs to a dedicated `postretro/ui` SDK namespace once
TypeScript and Luau can express that namespace with reasonable parity.

## Scope

### In scope

- Clean breaking SDK refactor.
- Singular theme groups: `color`, `font`, `spacing`.
- Nested theme authoring in TypeScript and Luau.
- `getDesignTokens(theme)` returning nested token groups.
- Category-branded token leaves so color, font, and spacing tokens cannot be
  mixed at widget call sites.
- Runtime descriptors and Rust theme resolution still receive flat token names.
- TypeScript UI imports from `postretro/ui`.
- Luau UI imports via `require("postretro/ui")`.
- Remove UI symbols from the public root `postretro` type surface.
- Update dev scripts, docs, generated types, and UI context docs.

### Out of scope

- General script import alias rewriting. Authors must use unaliased TypeScript
  named imports until the compiler rewrites imports.
- Renderer-side token aliasing.
- A CSS cascade or DOM-style theme layer.
- Supporting the old `colors` / `fonts` plural theme input.
- Supporting `theme.tokens.color("...")`, `tokens.font(...)`, or
  `tokens.spacing(...)`.
- Preserving direct string-token widget props in the SDK.

## Acceptance criteria

- [ ] TypeScript authors can write `import { Text, defineTheme } from "postretro/ui"`.
- [ ] Luau authors can write `local UI = require("postretro/ui")`.
- [ ] Root `postretro` type declarations no longer expose UI factories, layout
      helpers, UI tree helpers, UI state helpers, UI reactions, or theme helpers.
- [ ] Existing runtime descriptor wire remains unchanged: token-capable fields
      serialize as bare strings or literals.
- [ ] `defineTheme({ color, font, spacing })` flattens nested authoring tokens
      into the existing manifest theme shape consumed by Rust.
- [ ] `getDesignTokens(theme)` returns nested groups matching the authored
      theme shape.
- [ ] `font.primary`, `color.panel.default`, and `spacing.m` are valid token
      leaves in both TypeScript and Luau.
- [ ] Passing a color token to `font`, a font token to `color`, or a spacing
      token to `color` is a static type error in TypeScript and in the supported
      Luau type-checking path.
- [ ] Unknown token degradation in Rust stays unchanged: unknown color becomes
      magenta, unknown font falls back to the primary font, unknown spacing
      becomes zero, each warning once per tree build.
- [ ] Dev HUD, pause menu, reactive UI fixtures, and script-compiler tests use
      `postretro/ui` for UI APIs.

## Tasks

### Task 1: Split public module types from prelude globals

The TypeScript prelude still needs UI globals because imports are stripped
without alias rewriting. Separate the public module type surface from the
prelude installation surface. The prelude may keep exporting UI names solely to
install globals; `sdk/types/postretro.d.ts` should decide what the public
`postretro` module exposes. Keep this split explicit in comments so a later
compiler alias-rewrite plan can remove the extra globals safely.

### Task 2: Add `postretro/ui` module declarations

Generate a TypeScript `declare module "postretro/ui"` block containing the UI
SDK surface. Extend Luau declarations from the virtual-module plan so
`require("postretro/ui")` returns the UI module table. The module includes widget
factories, layout factories, `Tree`, `defineUiTree`, UI state helpers, UI
reactions, `getGameState`, `defineTheme`, and `getDesignTokens`.

### Task 3: Replace theme authoring helpers

Change `defineTheme` in TypeScript and Luau to accept singular nested groups.
It returns a manifest-compatible object whose enumerable fields are flat
`colors`, `fonts`, and `spacing` maps. The original nested token tree is retained
only as non-enumerable SDK metadata for `getDesignTokens`.

Flattening rules:

| Authored path | Runtime category | Runtime token |
|---|---|---|
| `color.critical` | `colors` | `critical` |
| `color.panel.default` | `colors` | `panel.default` |
| `color.focus.ring` | `colors` | `focus.ring` |
| `font.primary` | `fonts` | `primary` |
| `spacing.m` | `spacing` | `m` |

### Task 4: Brand token leaves and update widget props

TypeScript token leaves are branded strings. Luau token leaves may be branded
records if Luau cannot distinguish string aliases strongly enough. In either
language, widget and layout factories unwrap SDK token leaves into plain strings
before returning descriptors. Literal colors and spacing numbers remain valid
escape hatches.

Update every token-capable prop: text font/color, panel fill/border tint, button
style colors, style-range colors, stack/grid gap and padding, and any current UI
reaction or helper that accepts a token-capable UI value.

### Task 5: Update engine default token names

Rename the default font token `body` to `primary`. Keep `mono`. Update Rust
font fallback to use `primary`, and update tests/docs that name the required
font set. Color and spacing runtime token names remain stable except that dotted
semantic names are now authored through nested groups.

### Task 6: Migrate content and tests

Update shipped dev scripts and SDK fixtures to import UI APIs from
`postretro/ui`, use `getDesignTokens`, and stop passing raw token strings. Add
type-oriented fixtures for wrong-category token usage. Add script-compiler tests
that prove `postretro/ui` bare imports are stripped like `postretro` imports.

## Sequencing

**Phase 1 (sequential):** Task 1 — separates runtime global installation from
public module typing.

**Phase 2 (concurrent):** Task 2, Task 3 — module declarations and theme helper
runtime can advance independently after Task 1.

**Phase 3 (sequential):** Task 4 — consumes the token helper types from Task 3.

**Phase 4 (sequential):** Task 5 — updates engine fallback names after SDK props
can express the new token.

**Phase 5 (sequential):** Task 6 — migrates content and locks behavior with
tests.

## Rough sketch

Proposed TypeScript authoring shape:

```ts
import { Text, VStack, defineTheme, getDesignTokens } from "postretro/ui";

const theme = defineTheme({
  color: {
    critical: [0.71, 0.05, 0.09, 1],
    panel: { default: [0.018, 0.026, 0.039, 1] },
  },
  font: {
    primary: "Inter",
    mono: "JetBrains Mono",
  },
  spacing: {
    m: 8,
  },
});

const { color, font, spacing } = getDesignTokens(theme);

VStack({ gap: spacing.m }, [
  Text({ content: "HP", font: font.primary, color: color.critical }),
]);
```

Proposed Luau authoring shape:

```lua
local UI = require("postretro/ui")

local theme = UI.defineTheme({
  color = {
    critical = {0.71, 0.05, 0.09, 1},
    panel = { default = {0.018, 0.026, 0.039, 1} },
  },
  font = {
    primary = "Inter",
    mono = "JetBrains Mono",
  },
  spacing = {
    m = 8,
  },
})

local tokens = UI.getDesignTokens(theme)

UI.Text({
  content = "HP",
  font = tokens.font.primary,
  color = tokens.color.critical,
})
```

`crates/postretro/src/scripting/typedef.rs` is already above the split smell
threshold. If the implementation adds a second large SDK type block, split the
static SDK-lib type text before extending it.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Root SDK module | n/a | n/a | `postretro` | `require("postretro")` | n/a |
| UI SDK module | n/a | n/a | `postretro/ui` | `require("postretro/ui")` | n/a |
| Theme authoring input | n/a | n/a | `ThemeDefinition.color/font/spacing` | `ThemeDefinition.color/font/spacing` | n/a |
| Theme manifest output | `ModThemeTokens` / `ThemeDescriptor` | `colors` / `fonts` / `spacing` | enumerable `colors` / `fonts` / `spacing` | enumerable `colors` / `fonts` / `spacing` | n/a |
| Color token leaf | descriptor string | bare string | `ColorToken` | `ColorToken` | n/a |
| Font token leaf | descriptor string | bare string | `FontToken` | `FontToken` | n/a |
| Spacing token leaf | descriptor string | bare string | `SpacingToken` | `SpacingToken` | n/a |
| Primary font token | `primary` | `"primary"` | `font.primary` | `tokens.font.primary` | n/a |

## Open questions

None. TypeScript import aliases stay out of scope until the compiler rewrites
bare imports instead of only stripping them.
