# Player Options

> **Read this when:** wiring runtime player preferences (sensitivity, invert-Y, accessibility preferences, OS preference seeding, input bindings), adding new per-player settings, or building the settings menu.
> **Key invariant:** player options and game saves are two distinct stores with separate formats and lifecycles. Never merge them. Accessibility preferences are client-local presentation: simulation never reads them and they never replicate.
> **Related:** [Architecture Index](./index.md) · [Input](./input.md) · [Boot Sequence](./boot_sequence.md) · [UI](./ui.md) §4.1 (accessibility panel)

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
- **Corruption fallback** — a file that is not valid TOML logs a warning and falls back to in-memory defaults without overwriting the file. Decided, not yet built: parsing is tolerant per field — an unrecognized value in any field falls back for that field alone with a warning, and every other setting loads intact. Today an unknown enum value still fails the whole-document parse.
- **Device identity** — `player_id` is an optional opaque 16-byte device-local value. It is generated only by `Session::build` when a loadable settings file lacks it, then written through the same atomic save path. `PlayerOptions::load` remains deterministic and never generates one. On connect it becomes the opaque `PlayerClaimId` in the fixed-size connection claim; it is never authentication, parsed, or exposed to scripts. The host retains the claim only for a future seat reclaim; roster controls never contain a player id or display name. If settings cannot be loaded or saved, the client connects anonymously and cannot reclaim a prior seat.

---

## 3. Boot Position

Player options load during post-first-pixel `Session::build`, before the session's `InputSystem` is constructed. The loaded options seed input preferences (sensitivity, invert-Y) at startup. On first boot (no file present), the engine writes defaults atomically before continuing. Decided, not yet built: the OS preference reader starts at session build, and the splash holds for its first reply up to a bounded wait, so the first frames show OS-seeded values; a later reply applies as a live change (`boot_sequence.md` §1). Session construction also generates and atomically persists the missing device identity, including for an existing valid settings file created before that field shipped.

The runtime options bridge saves accepted menu changes after a deterministic 250 ms settle window. Closing the options menu or exiting cleanly flushes a pending write synchronously; failures warn, preserve the in-memory value and existing file, and a later change retries through the same atomic path.

---

## 4. E13 Settings Menu Seam

`PlayerOptions` is the store the settings menu reads and writes. The dev title screen opens `frontend.options`, whose seven controls cover mouse sensitivity, invert-Y, view-feel scale, crouch mode, shadow quality, fog quality, and the Surface Depth on/off switch.

**Seam mechanism.** The menu never writes `PlayerOptions` directly — the UI module originates no store write (`ui.md` §3). The engine exposes writable, non-persisted `options.*` slots on `getGameState()`, seeded from the current in-memory `PlayerOptions` when the menu opens. Controls write them via `setState` at the game-logic stage; the session-owned options bridge observes write generations once per app frame, updates only the matching `PlayerOptions` field, applies live input/fog/Surface-Depth effects through their owners, and schedules the settled atomic save. The slots are UI-facing working copies; `PlayerOptions` / `settings.toml` remains the authoritative persisted home, re-seeded into the slots on every open rather than maintained as a continuous two-way sync.

**Graphics quality lives here.** Player-facing graphics-quality tiers are `PlayerOptions` fields, not a renderer-side store. `shadow_quality` maps low/medium/high to 512/768/1024 spot-shadow pixels (default high); its setter changes CPU boot state only, and full renderer construction or the next level geometry install rebuilds the spot pool and resolution-coupled caches. `fog_quality` maps low/medium/high to 1.0/0.5/0.25 ray-march step size (default medium) and applies live through `Renderer::set_fog_step_size`; full init re-applies it. Fog quality never overrides the map-owned `fog_pixel_scale`. `surface_depth_quality` is **off/on** rather than a low/medium/high ladder (design D5) and defaults to **on**: the feature ships enabled and the setting is an escape hatch for hardware that struggles, not an opt-in. Two states because this is a pure **cost lever**, not a quality ladder — the middle tier it once had capped the march's step budget and halved its fade distance but never changed `depth_meters`, so it looked like the full effect except at grazing angles, where it read as a shallower carve. `On` is every material's per-prefix tuning verbatim with the full self-shadow budget; `Off` is byte-identical to the pre-feature render at zero cost. It applies live through `Renderer::set_surface_depth_quality`, which rewrites every installed material's uniform buffer in place rather than rebuilding bind groups (`rendering_pipeline.md` §7.3, `resource_management.md` §8.2); a change made with no level loaded is retained in renderer boot state and consumed by the next `install_textures`. All three translations live at the app render-profile chokepoint, so UI and option storage never own GPU work.

**Stale persisted enum values are player data, not an API shim.** `settings.toml` written before D5 collapsed carries `surface_depth_quality = "low"` or `"high"`; `"high"` is in every file that ever saved the default. Both named a state that *did* march, so both deserialize to `On` through `serde(alias)` and are rewritten as `"on"` on the next save. This is not the code-level backward compatibility `development_guide.md` §1.6 forbids: until per-field parsing lands (§2), an unknown enum value fails the whole-document TOML parse, which would discard every other setting in the file — sensitivity, invert-Y, crouch mode, the device identity — not just this one. The live `options.surfaceDepthQuality` slot vocabulary stays strictly `off`/`on`; the retired names are accepted only when reading a file.

---

## 5. Accessibility Preferences

`view_feel_scale` (`[0, 1]`, default `1.0`) is an accessibility scale for view-feel responsiveness. Clamped on load. Multiplies presented bob, tilt, sway, and state-transition FOV/pitch/roll impulses at render assembly. `0` suppresses all view-feel presentation; impulse integration continues.

Decided, not yet built: an `accessibility` group in `PlayerOptions` holds the player's accommodations — reduce motion and its per-effect scales, the flash limiter, bus volumes and mono, theme variant, text scale, captions and cues, and whichever input options their owners place there. `view_feel_scale` joins the group but keeps its top-level key. The panel (`ui.md` §4.1) carries every field.

**OS seeding and resolution.** An OS-seedable field stores *unset* until the player changes it. Resolution: **player-set > OS value > engine default**. OS change events update unset fields live; an OS change never overrides a player-set value. A menu write or panel action marks a field player-set; an OS value leaves it unset, and saving writes a never-changed field as unset, not as the value it resolved to. The OS reader sits behind an app-side seam and reads OS contrast (seeds the theme variant), reduced motion, and Windows text scale. OS flashing-lights, caption, mono-audio, and screen-reader flags have no safe API and are not read; those are in-game options only.

**Two slot surfaces.**
- `accessibility.*` — engine-owned, readonly, always live. Each carries its field's resolved value during play, menu open or not. Mod content reads it to honor a preference in its own content; no script writes it.
- `options.*` — the menu-scoped working copies of §4, unchanged in kind, for every accessibility field but the flash limiter. An engine write to the store — a panel action, the OS reader's first reply, an OS change to an unset field — reseeds the matching working copy that frame, and the reseed never reads back as a menu write. Session build also seeds every accessibility working copy once, so a mod menu under any tree name shows resolved values.

**Flash limiter.** On by default. It has no script-writable working copy: no `options.*` slot exists for it, `accessibility.flashLimiter` is readonly, and only the engine panel's own control changes it (`ui.md` §4.1, `rendering_pipeline.md` §7.8).

**Reduce motion.** One OS-seeded switch over per-effect sliders: view-feel scale, screen-shake scale, and snapping of UI and presentation-template tweens (`ui.md` §3). While on, each effect applies its reduced value; off, each follows its own slider, and slider values survive toggling the switch. Each per-effect `accessibility.*` slot carries its slider's value; the switch has its own slot.

**Client-local presentation.** Preferences apply only where presentation runs on the affected player's machine — UI, screen effects, audio, captions. Simulation never reads them, and no `accessibility.*` slot or accessibility working copy replicates. In co-op each machine applies its own settings to its own screen.

**First-launch record.** The store also records whether the panel has been shown; boot writes it when the player closes the panel (`boot_sequence.md` §1).

---

## 6. Input Bindings (decided, not yet built)

Bindings live in this store, per player, in `settings.toml`. Every action is remappable — keyboard/mouse and gamepad, UI navigation actions included. The action set stays engine-closed; players remap it, mods do not extend it.

- **Per-binding fallback.** An unknown key string falls back to that binding's default without discarding other bindings or settings.
- **Conflicts and reset.** A conflicting assignment is reported before it applies. Reset restores defaults.
- **Guards.** Confirm, cancel, and the accessibility panel's global input can be moved but never left unbound. The panel input's keyboard entry is never bound to a key text entry consumes (printable keys, Backspace, Enter, Escape): that input reads ahead of text entry, so the key could no longer be typed.
- **Capture.** A rebind capture receives raw input the UI otherwise swallows, the panel's global input included (`input.md` §5).

---

## 7. Non-Goals

- **Direct scripting access to the persisted store.** `PlayerOptions` and its Rust enums remain engine-internal; scripts receive only typed `options.*` state refs generated from the engine-state catalog, never the TOML store or save API.
- **Game-save store.** Separate spec.
