# Player Options

## Goal

An engine-owned store for per-human runtime preferences (mouse sensitivity, invert-Y, accessibility scales), persisted to a human-editable `settings.toml` in the platform config directory. Loads at boot, writes defaults when absent, saves atomically on change. This is the seam the future M13 settings menu drives — it ships the store and wiring, no UI.

Distinct from the game-save store. Player options are settings (input/audio/accessibility preferences); saves are game state. They are two separate stores with separate formats and lifecycles. This spec is only the options store.

## Scope

### In scope
- An engine-owned `PlayerOptions` serde struct: scalar/bool/enum preference fields, each with an explicit default matching today's hardcoded engine default.
- TOML persistence: load at boot, write defaults when the file is absent, atomic save (write-temp-then-rename). Path resolved under the platform config directory via the `directories` crate.
- Corruption tolerance: a malformed file logs a warning and falls back to in-memory defaults without overwriting the user's file.
- Schema evolution: every field is `serde(default)` so partial/older files load cleanly (missing fields fall back to defaults).
- Fold in the two existing unpersisted input preferences as the proof the store is the right home: `mouse_sensitivity` and `invert_y` (`input/mod.rs:143-146`), today hardcoded with test-only setters. Boot seeds `InputSystem` from `PlayerOptions`.
- A `view_feel_scale` field (`[0,1]`, default `1.0`) — the accessibility seam the `movement--view-feel` draft (D6) describes. Defined here; no consumer in this spec.

### Out of scope
- Any UI. The M13 settings menu reads/writes this store and renders the controls. No egui or UI pass work here.
- Keybind remapping. Same conceptual home, far larger surface (rebind flow, conflict detection, gamepad maps). v1 ships fixed bindings; remapping is its own later spec.
- The game-save store. Separate spec. (SQLite is a candidate *there* — relational/transactional/large state — and explicitly rejected for options.)
- `crouchMode` (toggle vs. hold). Crouch does not exist yet; the crouch spec adds that field as a consumer. The store is shaped so adding it is a one-field change, and the toggle-vs-hold *resolution* belongs in the input layer (deriving a crouch edge), never in the movement intent.
- Gamepad dead zone as an option. Currently a hardcoded `const` (`gamepad.rs:10`); `input.md` §6 already anticipates it as a preference. Natural next field, but folding it requires const→field plumbing in `gamepad.rs` — deferred to keep v1 tight.
- A scripting surface. Unlike movement/light descriptors, `PlayerOptions` is **not** script-facing: no SDK type emission, no `register_type`, no `.d.ts`/`.d.luau`, no drift test. It is engine-internal config.
- Save-on-change *triggers*. v1 has no runtime mutation source (no UI), so the only `save` caller today is the boot defaults-write. The M13 settings menu becomes the real save-on-change driver later.

## Acceptance criteria

- [ ] On boot with no settings file present, the engine loads defaults, writes a `settings.toml` containing those defaults to the platform config path, and input behavior is identical to today (sensitivity and invert-Y unchanged).
- [ ] On boot with a settings file present, its `mouse_sensitivity` and `invert_y` values are applied to the input system (a non-default sensitivity in the file changes look speed; `invert_y = true` in the file inverts pitch).
- [ ] A settings file missing one or more known fields loads successfully: present fields are applied, absent fields fall back to their defaults.
- [ ] A malformed (unparseable) settings file produces a logged warning, the engine runs on in-memory defaults, and the existing file is left unmodified (not overwritten).
- [ ] Saving writes atomically: the target file is replaced via a temp-file-then-rename, no partial/truncated file is observable, no `.tmp` artifact remains, and the written file parses back to an equal `PlayerOptions`.
- [ ] A `PlayerOptions` serialized to TOML and deserialized back yields an equal value (round-trip).
- [ ] The resolved settings path is under the platform config directory and named `settings.toml`.
- [ ] `view_feel_scale` is present in the struct and serialized file with default `1.0`; it is clamped to `[0,1]` on load (out-of-range values in the file are clamped, not rejected).
- [ ] Load and save are testable against an injected path (a temp directory), without touching the real user config directory or mutating process environment.
- [ ] The options module has no dependency on wgpu, the renderer, or any save/UI system (engine-infra boundary).

## Tasks

### Task 1: `PlayerOptions` data + TOML persistence module
Add a new engine module (`crates/postretro/src/options/mod.rs`) owning the `PlayerOptions` serde struct and its persistence. Fields v1: `mouse_sensitivity: f32` (default = the existing `DEFAULT_MOUSE_SENSITIVITY`), `invert_y: bool` (default `false`), `view_feel_scale: f32` (default `1.0`, clamped to `[0,1]` on load). Every field carries `#[serde(default = ...)]` so partial files load. Provide: a path resolver (`directories::ProjectDirs` → `<config_dir>/settings.toml`, returning the resolved `PathBuf`); `load(path) -> PlayerOptions` that returns defaults on a missing or malformed file (logging a warning on parse failure, leaving the file untouched) and clamps ranged fields; `save(&self, path) -> io::Result<()>` writing atomically (serialize to TOML, write a sibling temp file in the same directory, `fs::rename` over the target, creating the parent directory if absent). Add `toml` and `directories` as dependencies (workspace + crate manifest), `tempfile` as a dev-dependency. Unit-test against injected temp paths: round-trip, missing-file→defaults+written, partial-file→field defaults, malformed-file→defaults+untouched, atomic-save leaves no `.tmp` and parses back equal, clamp on out-of-range `view_feel_scale`. Pure data + fs; no wgpu.

### Task 2: Boot load + input fold-in
Wire the store into engine startup. In App init (`main.rs`, around the `InputSystem::new` at `:226`), resolve the path, `load` the options, and write defaults if the file was absent. Store the loaded `PlayerOptions` (and the resolved path) on `App` for future saves. After constructing `InputSystem`, seed it from the options via the existing `set_mouse_sensitivity` / `set_invert_y` setters — giving those setters their first non-test caller (drop the now-inaccurate `#[allow(dead_code)]` where a real caller exists). Confirm the defaults path leaves look/invert behavior bit-identical to today. No save-on-change caller is added (no UI yet); the boot defaults-write is the only `save` invocation.

## Sequencing

**Phase 1 (sequential):** Task 1 — the module and its persistence; nothing to wire until it exists.
**Phase 2 (sequential):** Task 2 — consumes Task 1's `PlayerOptions` and path resolver; folds input settings in at boot.

## Rough sketch

Module: `crates/postretro/src/options/mod.rs`, owned by the engine (`App` in `main.rs`), no wgpu — mirrors the input subsystem's GPU-free boundary so it stays unit-testable.

- `PlayerOptions` — `#[derive(Serialize, Deserialize, ...)]`, `#[serde(default)]` per field. Defaults via free functions referenced by `serde(default = "...")` (e.g. the sensitivity default returns `DEFAULT_MOUSE_SENSITIVITY`, re-exported or duplicated from `input`).
- Path: `ProjectDirs::from("", "", "postretro")` → `config_dir().join("settings.toml")` (Linux `~/.config/postretro/settings.toml`; the qualifier/org triple is a minor implementer call). Path resolution is separate from load/save so tests inject a temp path.
- `load(&Path)`: read → `toml::from_str`; on read-error treat as absent (return defaults, caller writes them); on parse-error log a warning and return defaults *without* touching the file; clamp `view_feel_scale`.
- `save(&self, &Path)`: `toml::to_string_pretty` → write `settings.toml.tmp` in the same dir → `fs::rename` over `settings.toml` (atomic on a single filesystem); `create_dir_all` the parent first.
- Boot: load once at App init; if the file was absent, `save` the defaults (this is the real exercise of the atomic-write path at boot). Seed `InputSystem` from the loaded values.

`confy` was considered (it bundles path + load/save-defaults) but its fixed internal path resolution complicates injecting a temp path for tests, and v1 wants explicit control over corruption fallback and atomic write — so a thin hand-rolled `directories` + `toml` path is preferred. Implementer may revisit.

Coordination with `movement--view-feel`: that draft (D6) plans a "hardcoded-but-seamed" const for the accessibility scale. Neither is built yet. If player-options lands first (the intended order), view-feel reads `PlayerOptions::view_feel_scale` directly instead of minting its own const — the seam is this field. Note for whoever implements view-feel.

## Wire format

TOML, not a binary/PRL section — no endianness or byte-layout concerns. Serde naming: snake_case keys (the serde default, matching Rust field names) — **no** `rename_all`. This deliberately differs from the project's camelCase *script-facing* wire convention: a settings file is a human-editable config for the player, not a script object, and snake_case is the idiomatic Rust/TOML config style.

| Rust field | TOML key | Type | Default | Range |
|---|---|---|---|---|
| `mouse_sensitivity` | `mouse_sensitivity` | float | `DEFAULT_MOUSE_SENSITIVITY` | finite > 0 |
| `invert_y` | `invert_y` | bool | `false` | — |
| `view_feel_scale` | `view_feel_scale` | float | `1.0` | clamped `[0,1]` on load |

Schema evolution: every field `serde(default)`, so an older/partial file loads with absent fields defaulted. Unknown keys (from a newer binary's file read by an older one) are dropped on the next rewrite — acceptable for v1; preserving unknown keys is a future refinement if cross-version clobbering becomes a real concern.

## Open questions
- **Settings-vs-saves boundary (decided, recorded here for the save-store spec).** Two distinct stores. The save store is out of scope; SQLite is a candidate there, rejected here. Flagged so the future save-store spec doesn't try to merge them.
- **`directories` triple.** Exact qualifier/organization/application strings for `ProjectDirs::from` are a minor implementer choice; only the `settings.toml` filename and "under the platform config dir" are pinned.
