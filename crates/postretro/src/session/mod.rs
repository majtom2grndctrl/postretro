// Live session-lifetime runtime container, built after the first visible frame.
// Owns the input/UI/modal field group migrated off the `App` god-struct so boot
// code cannot name a session field before install.
// See: context/lib/boot_sequence.md §1 (Deferred-session boundary and single commit)

use anyhow::Result;

use crate::input;
use crate::render;

/// Live session-lifetime container, held on `App` as `Option<Session>` and built
/// once after first pixels by [`Session::build`]. Owns the input/UI/modal field
/// group: every field here is session-lifetime, and none can be named while
/// `App.session` is `None` (boot phase). The opposite of `SessionServices` (a
/// transient pre-window construction bundle that `build_session` destructures and
/// discards) — `Session` is the live runtime owner.
///
/// This is the Task-1 migrated group only. `script_ctx`/`script_runtime` (Task 2)
/// and `net_endpoint`/`audio` (Task 3) still live on `App` until their migrations
/// land; `PendingSessionInit` carries the inputs for both construction sites until
/// Task 3 collapses them. The group holds no `ScriptCtx` clone or registry handle,
/// so it is severable from the script tranche and buildable in isolation.
/// See: context/lib/boot_sequence.md §1.
pub(crate) struct Session {
    /// Keyboard/mouse/gamepad action state. Seeded at build with the loaded
    /// look preferences. See: context/lib/input.md
    pub(crate) input_system: input::InputSystem,

    /// Per-tick gameplay-input latch; neutralized while a modal captures input.
    pub(crate) gameplay_input_latch: input::GameplayInputLatch,

    /// Input-stage UI-dispatch seam (capture vs. passthrough).
    /// See: context/lib/input.md
    pub(crate) ui_dispatch: input::UiDispatch,

    /// Gamepad subsystem. Inner `Option` encodes runtime absence (no pad / gilrs
    /// init failure) — distinct from "session not yet installed."
    pub(crate) gamepad_system: Option<input::gamepad::GamepadSystem>,

    /// Coarse keyboard/mouse focus owner. Drives pointer-lock acquire/release.
    /// See: context/lib/input.md
    pub(crate) input_focus: input::InputFocus,

    /// UI focus engine: moves focus through the top stack tree, runs the
    /// hold-to-repeat clock, yields the focused node id. See: context/lib/ui.md §4.
    pub(crate) ui_focus: input::UiFocusEngine,

    /// The focus rect list the renderer exported for the top tree LAST frame.
    /// Inner `Option` encodes "not exported yet," not "session not installed."
    /// See: context/lib/ui.md §4.
    pub(crate) ui_focus_rects: Option<render::ui::tree::FocusRectList>,

    /// Pointer-vs-focus interaction mode (hover moves focus only in `Pointer`).
    /// See: context/lib/input.md §7.
    pub(crate) ui_input_mode: input::InputMode,

    /// Gameplay-UI modal stack + named-tree registry. Built-in trees register at
    /// build. See: context/lib/ui.md §1.
    pub(crate) modal_stack: render::ui::modal_stack::ModalStack,
}

/// Look-preference seed for `InputSystem`, captured pre-window from the loaded
/// `PlayerOptions` (which stays on `App` until a later task) and carried through
/// `PendingSessionInit` to the post-first-pixel `Session::build`. Keeping the two
/// scalars (not a `PlayerOptions` borrow) keeps the migrated build independent of
/// the not-yet-migrated `player_options` field — no construction dependency
/// crosses the dual-construction boundary. See: context/lib/player_options.md §3.
pub(crate) struct InputSeed {
    pub(crate) mouse_sensitivity: f32,
    pub(crate) invert_y: bool,
}

impl Session {
    /// Build the migrated input/UI/modal session group AFTER the first visible
    /// frame, synchronously and whole-or-nothing. Runs entirely within the single
    /// install redraw — no `await`, no yield. Absorbs the migrated-field work that
    /// previously ran in `install_pending_session` / `install_post_splash_services`
    /// and the pre-window `build_session` literal.
    ///
    /// The only fallible step is the built-in UI tree registration's disk loads;
    /// those degrade per-tree (a missing/malformed asset warns and skips that
    /// screen) rather than failing the build, so `Session::build` returns `Err`
    /// only if a future step adds a hard failure. The `Result` signature is the
    /// contract the install path relies on. See: context/lib/boot_sequence.md §1.
    pub(crate) fn build(input_seed: &InputSeed) -> Result<Self> {
        let mut input_system = input::InputSystem::new(input::default_bindings());
        input_system.set_mouse_sensitivity(input_seed.mouse_sensitivity);
        input_system.set_invert_y(input_seed.invert_y);

        // Register engine built-in trees through the one shared load-and-register
        // path (`tree_asset::register_tree_from_disk`): each built-in screen's
        // `AnchoredTree` is authored in `content/base/ui/<file>.json` and loaded
        // from disk so a layout edit + reload changes it with no Rust change. A
        // missing/malformed asset warns once and skips the registration — that
        // screen is unavailable, the engine still runs.
        //
        // The HUD registers under `HUD_NAME` and resolves as the always-on bottom
        // passthrough layer each frame. The pause menu, frontend menu, and
        // keyboard register as pushed-only modals.
        let mut modal_stack = render::ui::modal_stack::ModalStack::new();
        {
            let registry = modal_stack.registry_mut();
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::tree_asset::HUD_NAME,
                "hud.json",
                true,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::demo::PAUSE_MENU_NAME,
                "pauseMenu.json",
                false,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::demo::FRONTEND_MENU_NAME,
                "frontendMenu.json",
                false,
            );
            render::ui::tree_asset::register_tree_from_disk(
                registry,
                render::ui::keyboard_asset::KEYBOARD_TREE_NAME,
                "keyboard.json",
                false,
            );
        }

        Ok(Self {
            input_system,
            gameplay_input_latch: input::GameplayInputLatch::new(),
            ui_dispatch: input::UiDispatch::new(),
            gamepad_system: input::gamepad::GamepadSystem::new(),
            input_focus: input::InputFocus::Gameplay,
            ui_focus: input::UiFocusEngine::new(),
            ui_focus_rects: None,
            ui_input_mode: input::InputMode::default(),
            modal_stack,
        })
    }
}
