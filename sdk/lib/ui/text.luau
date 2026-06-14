--!strict
-- UI text-alias chokepoint (M13 G1a). `LocalizedText` is the single type of
-- every user-facing text prop on UI widgets. It is `string` today; a future
-- localization swap (e.g. to a message-key handle) is one edit here.
-- Type-only -- no runtime global is promoted.
-- See: context/lib/ui.md · context/lib/scripting.md §7

--- The type of every user-facing text string a widget displays. A single alias
--- (`= string` today) so a future localization scheme -- message keys, ICU
--- handles -- is one edit, not a sweep across every text prop.
export type LocalizedText = string

return {}
