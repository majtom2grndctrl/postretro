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
- **Device identity** — `player_id` is an optional opaque 16-byte device-local value. It is generated only by `Session::build` when a loadable settings file lacks it, then written through the same atomic save path. `PlayerOptions::load` remains deterministic and never generates one. On connect it becomes the opaque `PlayerClaimId` in the fixed-size connection claim; it is never authentication, parsed, or exposed to scripts. The host retains the claim only for a future seat reclaim; roster controls never contain a player id or display name. If settings cannot be loaded or saved, the client connects anonymously and cannot reclaim a prior seat.

---

## 3. Boot Position

Player options load during post-first-pixel `Session::build`, before the session's `InputSystem` is constructed. The loaded options seed input preferences (sensitivity, invert-Y) at startup. On first boot (no file present), the engine writes defaults atomically before continuing. Session construction also generates and atomically persists the missing device identity, including for an existing valid settings file created before that field shipped.

The runtime options bridge saves accepted menu changes after a deterministic 250 ms settle window. Closing the options menu or exiting cleanly flushes a pending write synchronously; failures warn, preserve the in-memory value and existing file, and a later change retries through the same atomic path.

---

## 4. E13 Settings Menu Seam

`PlayerOptions` is the store the settings menu reads and writes. The dev title screen opens `frontend.options`, whose seven controls cover mouse sensitivity, invert-Y, view-feel scale, crouch mode, shadow quality, fog quality, and Surface Depth quality.

**Seam mechanism.** The menu never writes `PlayerOptions` directly — the UI module originates no store write (`ui.md` §3). The engine exposes writable, non-persisted `options.*` slots on `getGameState()`, seeded from the current in-memory `PlayerOptions` when the menu opens. Controls write them via `setState` at the game-logic stage; the session-owned options bridge observes write generations once per app frame, updates only the matching `PlayerOptions` field, applies live input/fog/Surface-Depth effects through their owners, and schedules the settled atomic save. The slots are UI-facing working copies; `PlayerOptions` / `settings.toml` remains the authoritative persisted home, re-seeded into the slots on every open rather than maintained as a continuous two-way sync.

**Graphics quality lives here.** Player-facing graphics-quality tiers are `PlayerOptions` fields, not a renderer-side store. `shadow_quality` maps low/medium/high to 512/768/1024 spot-shadow pixels (default high); its setter changes CPU boot state only, and full renderer construction or the next level geometry install rebuilds the spot pool and resolution-coupled caches. `fog_quality` maps low/medium/high to 1.0/0.5/0.25 ray-march step size (default medium) and applies live through `Renderer::set_fog_step_size`; full init re-applies it. Fog quality never overrides the map-owned `fog_pixel_scale`. `surface_depth_quality` is **off/low/high** rather than low/medium/high — the meaningful choices are "no march", "the primary march only", and "the full effect" — and defaults to **high**: the feature ships on and the tier is an escape hatch for hardware that struggles, not an opt-in. It applies live through `Renderer::set_surface_depth_quality`, which rewrites every installed material's uniform buffer in place rather than rebuilding bind groups (`rendering_pipeline.md` §7.3, `resource_management.md` §8.2); a change made with no level loaded is retained in renderer boot state and consumed by the next `install_textures`. All three translations live at the app render-profile chokepoint, so UI and option storage never own GPU work.

---

## 5. `view_feel_scale` Seam

`view_feel_scale` (`[0, 1]`, default `1.0`) is an accessibility scale for view-feel responsiveness. Clamped on load. Multiplies presented bob, tilt, sway, and state-transition FOV/pitch/roll impulses at render assembly. `0` suppresses all view-feel presentation; impulse integration continues.

---

## 6. Non-Goals

- **Keybind remapping.** Same conceptual home, separate spec (conflict detection, gamepad maps).
- **Direct scripting access to the persisted store.** `PlayerOptions` and its Rust enums remain engine-internal; scripts receive only typed `options.*` state refs generated from the engine-state catalog, never the TOML store or save API.
- **Game-save store.** Separate spec.
