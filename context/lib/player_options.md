# Player Options

> **Read this when:** wiring runtime player preferences (sensitivity, invert-Y, accessibility scales), adding new per-player settings, or building the settings menu.
> **Key invariant:** player options and game saves are two distinct stores with separate formats and lifecycles. Never merge them.
> **Related:** [Architecture Index](./index.md) · [Input](./input.md) · [Boot Sequence](./boot_sequence.md)

---

## 1. Two-Store Boundary

Player options are settings (input preferences, accessibility scales). Game saves are game state (level progress, inventory, player position). They are separate stores:

| Store | Format | Lifecycle |
|-------|--------|-----------|
| Player options | TOML, human-editable | Persists across sessions; survives game save deletion |
| Game saves | TBD (SQLite candidate) | Per-save-slot; separate spec |

Do not merge these stores. The formats, lifecycles, and ownership are incompatible.

---

## 2. Format and Persistence

Options persist as `settings.toml` in the platform config directory (e.g. `~/.config/postretro/settings.toml` on Linux). Format rationale:

- **TOML** — human-editable config. Snake_case keys (not camelCase — this is a player config file, not a script object).
- **Schema evolution** — every field carries `serde(default)`, so partial or older files load cleanly; absent fields fall back to defaults.
- **Atomic write** — serialize to a sibling `.tmp` file, then rename over the target. No partial/truncated writes are observable.
- **Corruption fallback** — malformed files log a warning and fall back to in-memory defaults without overwriting the file.

---

## 3. Boot Position

Player options load at engine init (Phase 0), before `InputSystem` is constructed. The loaded options seed input preferences (sensitivity, invert-Y) at startup. On first boot (no file present), the engine writes defaults atomically before continuing — the boot defaults-write is the primary exercise of the atomic-write path.

No save-on-change occurs at runtime until the E13 settings menu is wired. The boot defaults-write is the only save call before that.

---

## 4. E13 Settings Menu Seam

`PlayerOptions` is the store the E13 settings menu reads and writes. The options module ships the store and its boot wiring; the menu ships the UI, controls, and save-on-change triggers. These are distinct deliverables.

---

## 5. `view_feel_scale` Seam

`view_feel_scale` (`[0, 1]`, default `1.0`) is an accessibility scale for view-feel responsiveness. Clamped on load. Passed as the `global_scale` argument to `view_feel::evaluate` at the render-assembly site — multiplies all view-feel output (bob, tilt, sway). `0` zeroes all view feel.

---

## 6. Non-Goals

- **UI.** The E13 settings menu is a separate deliverable.
- **Keybind remapping.** Same conceptual home, separate spec (conflict detection, gamepad maps).
- **Scripting surface.** `PlayerOptions` is engine-internal config — no SDK types, no `.d.ts`/`.d.luau`, no drift test.
- **Game-save store.** Separate spec.
