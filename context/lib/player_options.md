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
- **Corruption fallback** — a file that is not valid TOML logs a warning and falls back to in-memory defaults without overwriting the file. Parsing is tolerant per field: each field loads on its own out of the parsed document, and a value this build cannot read falls back to that field's default alone, with a warning, while every other setting loads intact. A file that is not valid TOML is never overwritten, by a save or by the first-launch record.
- **Round-trip save.** Saving rewrites the loaded document, not a fresh serialization of the store. An unrecognized value, or a key this build does not know, survives until the player writes that field, so a newer build's settings outlive a trip through an older one; writing that field then replaces the stale text. A retired alias this build still reads is rewritten to its current name (§4).
- **Device identity** — `player_id` is an optional opaque 16-byte device-local value. It is generated only by `Session::build` when a loadable settings file lacks it, then written through the same atomic save path. `PlayerOptions::load` remains deterministic and never generates one. On connect it becomes the opaque `PlayerClaimId` in the fixed-size connection claim; it is never authentication, parsed, or exposed to scripts. The host retains the claim only for a future seat reclaim; roster controls never contain a player id or display name. If settings cannot be loaded or saved, the client connects anonymously and cannot reclaim a prior seat.

---

## 3. Boot Position

Player options load during post-first-pixel `Session::build`, before the session's `InputSystem` is constructed. The loaded options seed input preferences (sensitivity, invert-Y) at startup. On first boot (no file present), the engine writes defaults atomically before continuing. The OS preference reader starts at session build, after the first splash frame has presented, and never blocks a frame. Whatever follows mod init waits for its first reply up to 150 ms from the end of mod init, so the first frames show OS-seeded values; a later reply applies as a live change (`boot_sequence.md` §1). Session construction also generates and atomically persists the missing device identity, including for an existing valid settings file created before that field shipped.

The runtime options bridge saves accepted menu changes after a deterministic 250 ms settle window. Closing the options menu or exiting cleanly flushes a pending write synchronously; failures warn, preserve the in-memory value and existing file, and a later change retries through the same atomic path.

---

## 4. E13 Settings Menu Seam

`PlayerOptions` is the store the settings menu reads and writes. The dev title and pause menus open `frontend.options`, whose controls and graphics tabs cover mouse sensitivity, invert-Y, view-feel scale, crouch mode, shadow quality, fog quality, the Surface Depth on/off switch, and render resolution. Its accessibility tab carries every field of the accessibility group, as the engine panel does (`ui.md` §4.1): its toggles — reduce motion, the flash limiter, mono — fire the panel's field actions, which the App applies to the store; its sliders write working copies.

**Seam mechanism.** The menu never writes `PlayerOptions` directly — the UI module originates no store write (`ui.md` §3). The engine exposes writable, non-persisted `options.*` slots on `getGameState()`, seeded from the current in-memory `PlayerOptions` when the menu opens. Controls write them via `setState` at the game-logic stage; the session-owned options bridge observes write generations once per app frame, updates only the matching `PlayerOptions` field, applies live input/fog/Surface-Depth/render-resolution effects through their owners, and schedules the settled atomic save. The slots are UI-facing working copies; `PlayerOptions` / `settings.toml` remains the authoritative persisted home, re-seeded into the slots on every open rather than maintained as a continuous two-way sync.

**Graphics quality lives here.** Player-facing graphics-quality tiers are `PlayerOptions` fields, not a renderer-side store. `shadow_quality` maps low/medium/high to 512/768/1024 spot-shadow pixels (default high); its setter changes CPU boot state only, and full renderer construction or the next level geometry install rebuilds the spot pool and resolution-coupled caches. `fog_quality` maps low/medium/high to 1.0/0.5/0.25 ray-march step size (default medium) and applies live through `Renderer::set_fog_step_size`; full init re-applies it. Fog quality never overrides the map-owned `fog_pixel_scale`. `surface_depth_quality` is **off/on** rather than a low/medium/high ladder (design D5) and defaults to **on**: the feature ships enabled and the setting is an escape hatch for hardware that struggles, not an opt-in. Two states because this is a pure **cost lever**, not a quality ladder — the middle tier it once had capped the march's step budget and halved its fade distance but never changed `depth_meters`, so it looked like the full effect except at grazing angles, where it read as a shallower carve. `On` is every material's per-prefix tuning verbatim with the full self-shadow budget; `Off` is byte-identical to the pre-feature render at zero cost. It applies live through `Renderer::set_surface_depth_quality`, which rewrites every installed material's uniform buffer in place rather than rebuilding bind groups (`rendering_pipeline.md` §7.3, `resource_management.md` §8.2); a change made with no level loaded is retained in renderer boot state and consumed by the next `install_textures`. Every graphics translation, render resolution's included (below), lives at the app render-profile chokepoint, so UI and option storage never own GPU work.

**Stale persisted enum values are player data, not an API shim.** `settings.toml` written before D5 collapsed carries `surface_depth_quality = "low"` or `"high"`; `"high"` is in every file that ever saved the default. Both named a state that *did* march, so both deserialize to `On` through `serde(alias)` and are rewritten as `"on"` on the next save. This is not the code-level backward compatibility `development_guide.md` §1.6 forbids: per-field parsing (§2) means an unrecognized enum value falls back only for that one field, not the whole document — but the retired names still deserve their own aliases rather than a silent fallback, since both are player data naming a state the field genuinely had. The live `options.surfaceDepthQuality` slot vocabulary stays strictly `off`/`on`; the retired names are accepted only when reading a file.

**Render resolution.** Auto / Native / ½ / ⅓ / ¼ — integer divisors only, each dividing the surface into the scene extent (`rendering_pipeline.md` §7.8); default Auto. It persists as the top-level `render_resolution` key (`auto`, `native`, `half`, `third`, `quarter`; an unknown value falls back to Auto for that field alone), applies live, and is never a per-map key. The options screen reaches it through `options.renderResolution`, with the same five values, like the other graphics rows. `renderer_render_resolution` in `startup/render_profile.rs` maps it with an exhaustive match; the renderer only records the result, and the frame's extent commit applies it. Full init re-applies the saved value before building any scene target. Auto's divisor is max(1, floor(scale factor), ceil(surface height / 1440)): logical resolution on HiDPI, native on 1× displays up to 1440 rows, and larger panels capped at 1440 scene rows or fewer. Auto bounds visible pixel size; it does not tune the default for floor hardware. The 1440-row cap (`AUTO_MAX_SCENE_ROWS`) is player-options policy that reaches the renderer only through the chokepoint, and a source scan keeps it out of the renderer crate; the renderer combines it with the surface size and the window's scale factor, read at renderer build and on every scale-factor change.

---

## 5. Accessibility Preferences

`view_feel_scale` (`[0, 1]`, default `1.0`) is an accessibility scale for view-feel responsiveness. Clamped on load. Multiplies presented bob, tilt, sway, and state-transition FOV/pitch/roll impulses and sustained slide dip/FOV at render assembly. `0` suppresses all view-feel presentation; impulse integration continues.

An `accessibility` group in `PlayerOptions` holds the player's accommodations: reduce motion and its per-effect scales, the flash limiter, and bus volumes and mono. `view_feel_scale` joins the group but keeps its top-level key. Theme variant, text scale, captions and cues, and remapped input bindings are further accommodations their owning units add to the same group later (§6; `ui.md` §1, §2). The panel (`ui.md` §4.1) carries every field the group currently holds.

**OS seeding and resolution.** An OS-seedable field stores *unset* until the player changes it. Resolution: **player-set > OS value > engine default**. OS change events update unset fields live; an OS change never overrides a player-set value. A menu write or panel action marks a field player-set, even one that writes the value the field already resolves to; an OS value leaves it unset. Unset is an absent key: saving never writes a resolved value, and an unrecognized stored value in an OS-seedable field resolves as unset. An OS-seedable toggle cycles System → On → Off, so a player can always return to following the OS. Reduce motion is the one OS-seedable field today. The OS reader sits behind an app-side seam, polled once per frame; it reads reduced motion, OS contrast, and Windows text scale, but only reduced motion resolves an accessibility field so far — contrast and text scale are read into the same feed for the theme variant and text scale to consume once those ship (`ui.md` §1, §2). OS flashing-lights, caption, mono-audio, and screen-reader flags have no safe API and are not read; those are in-game options only.

**Two slot surfaces.**
- `accessibility.*` — engine-owned, readonly, always live. Each carries its field's resolved value during play, menu open or not. Mod content reads it to honor a preference in its own content; no script writes it.
- `accessibility.<field>FollowsSystem` — one readonly, unreplicated bool per OS-seedable field, true while the field is unset. Resolved slots stay resolved; this slot is how the panel and mod menus show a System choice ("System (On)").
- `options.*` — the menu-scoped working copies of §4, unchanged in kind, for every accessibility field but the flash limiter. An engine write to the store — a panel action, the OS reader's first reply, an OS change to an unset field — reseeds the matching working copy that frame, and the reseed never reads back as a menu write. Session build also seeds every accessibility working copy once, so a mod menu under any tree name shows resolved values.

**Flash limiter.** On by default. It has no script-writable working copy: no `options.*` slot exists for it and `accessibility.flashLimiter` is readonly. Only a button activation of its field action changes it — the engine panel's control or a mod or level menu's button; no reaction, manifest field, or script write reaches it (`ui.md` §4.1, `rendering_pipeline.md` §7.8).

**Reduce motion.** One OS-seeded switch over per-effect sliders: view-feel scale, screen-shake scale, and snapping of UI and presentation-template tweens (`ui.md` §3). While on, suppression is full: screen shake and view feel apply 0, and tweens reach their targets the frame they start, a running tween included. Off, each effect follows its own slider, and slider values survive toggling the switch. Scaling applies where the effect is presented and never writes a slot a script reads, so an effect decaying under the switch resumes at its decayed level when the switch turns off. Each per-effect `accessibility.*` slot carries its slider's value; the switch has its own slot.

**Client-local presentation.** Preferences apply only where presentation runs on the affected player's machine — UI, screen effects, audio, captions. Simulation never reads them, and no `accessibility.*` slot or accessibility working copy replicates. In co-op each machine applies its own settings to its own screen.

**Storage shape.** The accessibility fields live in an `[accessibility]` table; `view_feel_scale` keeps its top-level key. Scales and bus volumes clamp into `[0, 1]` on load. Bus volumes are linear `[0, 1]`, default 1.0, step 0.05 on the panel, mapped to decibels at the audio seam (`audio.md` §1); the App applies them at session build and on each resolved change.

**Every panel write persists like a menu write.** A panel action — the limiter's included — writes the store, marks the field written, and schedules the bridge's settled save; closing the panel flushes a pending save. A file that cannot be replaced (unreadable, or not valid TOML) takes no write: the save is a no-op, and the in-memory value still applies for the session.

**First-launch record.** The store also records, as the top-level `accessibility_panel_shown`, whether the panel has been shown. The key is absent until true. Boot writes it when the player closes the panel by any close path (`boot_sequence.md` §1).

---

## 6. Input Bindings (decided, not yet built)

Bindings live in this store, per player, in `settings.toml`. Every action is remappable — keyboard/mouse and gamepad, UI navigation actions included. The action set stays engine-closed; players remap it, mods do not extend it.

- **Per-binding fallback.** An unknown key string falls back to that binding's default without discarding other bindings or settings.
- **Conflicts and reset.** A conflicting assignment is reported before it applies. Reset restores defaults.
- **Guards.** Confirm and cancel can be moved but never left unbound.
- **Capture.** A rebind capture receives raw input the UI otherwise swallows (`input.md` §5).

---

## 7. Non-Goals

- **Direct scripting access to the persisted store.** `PlayerOptions` and its Rust enums remain engine-internal; scripts receive only typed `options.*` state refs generated from the engine-state catalog, never the TOML store or save API.
- **Game-save store.** Separate spec.
