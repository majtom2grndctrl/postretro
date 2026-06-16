--!strict
-- Luau parity reference for the M13 Goal C demo `intro` store. This is the
-- cross-runtime twin of `intro-store.ts`: the IDENTICAL `defineStore("intro", …)`
-- declaration expressed in Luau, documenting TS + Luau parity per the plan. Only
-- the TS `start-script.ts` is the active mod entry (the engine rejects having both
-- a `.ts` and `.luau` start-script), so this file is a reference module, not a
-- second active start-script.
--
-- `flashColor` is a length-4 linear-RGBA array (`{r, g, b, a}`). `persist = false`
-- keeps it out of the save file — a transient level-load animation slot, not
-- durable player state. Wire casing is camelCase across TS/JS/Luau (`flashColor`,
-- `type`, `default`, `persist`); the schema crosses the bridge to the same serde
-- shape as the TS form. `defineStore` is a pure builder; the active entry would
-- return `introStore.declaration` from `setupMod().stores`.
--
-- See: context/lib/scripting.md §3

local M = {}

M.introStore = defineStore("intro", {
	flashColor = { type = "array", default = { 0.0, 0.65, 0.75, 1.0 }, persist = false },
})

return M
