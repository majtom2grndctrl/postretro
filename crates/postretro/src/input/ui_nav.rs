// UI navigation-intent vocabulary and the action→intent mapping the input
// stage feeds into the UI-dispatch queue.
// See: context/lib/input.md §7 · context/research/ui-layer.md §16

//! The nav-intent layer maps fixed physical inputs — keyboard arrows/enter/
//! escape, gamepad D-pad and face/system buttons, and stick-past-deadzone edges
//! — to a closed [`NavIntent`] vocabulary. This is deliberately *not* routed
//! through the remappable [`Action`](crate::input::Action) binding table: UI nav
//! reads fixed inputs; remapping stays the action-map layer's concern (M13 Goal
//! F scope). The intents this module produces are wrapped in
//! [`UiIntent::Nav`](crate::input::ui_dispatch::UiIntent) and ride the existing
//! N→N+1 [`UiDispatch`](crate::input::ui_dispatch::UiDispatch) queue.
//!
//! ## Escape routing seam
//!
//! Escape is `nav.menu` from gameplay but `nav.cancel` inside a capturing UI
//! tree. The "is a capturing tree on the stack?" predicate is the UI-dispatch
//! seam's `Capture` mode, which `App::reconcile_ui_focus` sets from the modal
//! stack's top capture mode (M13 Goal F). The App threads it through
//! [`nav_intent_for_key`] as `capturing_tree_present`.

use gilrs::Button as GilrsButton;
use winit::keyboard::KeyCode;

/// Closed UI-navigation intent vocabulary. Each variant carries a stable wire
/// name (`"nav.up"` … `"nav.options"`) consumed by JSON/TS/Luau UI authors
/// (`capturesNav`, focus policy). New variants extend the [`wire_name`] match
/// and the TS/Luau union in `scripting::typedef` in lockstep.
///
/// [`wire_name`]: NavIntent::wire_name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavIntent {
    Up,
    Down,
    Left,
    Right,
    /// Advance focus to the next sibling (Tab / shoulder-button forward).
    Next,
    /// Retreat focus to the previous sibling.
    Prev,
    /// Activate the focused widget (Enter / A / South).
    Confirm,
    /// Dismiss/back out within a capturing tree (Escape-inside-UI / B / East).
    Cancel,
    /// Open or toggle the menu (Start / Escape-from-gameplay).
    Menu,
    /// Open the options/back surface (Select / Back).
    Options,
}

impl NavIntent {
    /// The stable wire name for this intent, matching the TS template-literal
    /// type and Luau string union emitted in the SDK typedefs. The UI authoring
    /// surface (`capturesNav`, focus policy) keys on these strings.
    ///
    /// The slider nav-capture path (M13 Goal F, Task 4) matches authored
    /// `capturesNav` wire names against these to claim captured nav intents.
    pub fn wire_name(self) -> &'static str {
        match self {
            NavIntent::Up => "nav.up",
            NavIntent::Down => "nav.down",
            NavIntent::Left => "nav.left",
            NavIntent::Right => "nav.right",
            NavIntent::Next => "nav.next",
            NavIntent::Prev => "nav.prev",
            NavIntent::Confirm => "nav.confirm",
            NavIntent::Cancel => "nav.cancel",
            NavIntent::Menu => "nav.menu",
            NavIntent::Options => "nav.options",
        }
    }
}

/// Map a keyboard key press to a nav intent, or `None` for keys the UI nav
/// vocabulary ignores. Only key-*down* edges should call this; held repeats are
/// the focus engine's hold-to-repeat concern (Task 3), not a fresh intent.
///
/// Escape routing depends on `capturing_tree_present`: from gameplay (no
/// capturing tree) Escape opens the menu (`nav.menu`); inside a capturing tree
/// it backs out (`nav.cancel`). The App sources the flag from the UI-dispatch
/// seam's `Capture` mode (set from the modal stack's top capture mode).
pub fn nav_intent_for_key(key: KeyCode, capturing_tree_present: bool) -> Option<NavIntent> {
    Some(match key {
        KeyCode::ArrowUp => NavIntent::Up,
        KeyCode::ArrowDown => NavIntent::Down,
        KeyCode::ArrowLeft => NavIntent::Left,
        KeyCode::ArrowRight => NavIntent::Right,
        KeyCode::Tab => NavIntent::Next,
        KeyCode::Enter | KeyCode::NumpadEnter => NavIntent::Confirm,
        KeyCode::Escape => {
            if capturing_tree_present {
                NavIntent::Cancel
            } else {
                NavIntent::Menu
            }
        }
        _ => return None,
    })
}

/// Map a gamepad button to a nav intent, or `None` for buttons outside the UI
/// nav vocabulary. Only button-*down* edges should call this.
///
/// Bindings (M13 Goal F): South = confirm, East = cancel, D-pad = directions,
/// shoulders = next/prev, Start = `nav.menu`, Select = `nav.options`.
pub fn nav_intent_for_gamepad_button(button: GilrsButton) -> Option<NavIntent> {
    Some(match button {
        GilrsButton::DPadUp => NavIntent::Up,
        GilrsButton::DPadDown => NavIntent::Down,
        GilrsButton::DPadLeft => NavIntent::Left,
        GilrsButton::DPadRight => NavIntent::Right,
        GilrsButton::RightTrigger => NavIntent::Next,
        GilrsButton::LeftTrigger => NavIntent::Prev,
        GilrsButton::South => NavIntent::Confirm,
        GilrsButton::East => NavIntent::Cancel,
        GilrsButton::Start => NavIntent::Menu,
        GilrsButton::Select => NavIntent::Options,
        _ => return None,
    })
}

/// Edge detector that turns a continuous nav stick into discrete D-pad-style nav
/// intents: pressing a stick past the dead zone in one of the four cardinal
/// directions emits exactly one intent per crossing, and the stick must return
/// inside the dead zone before it can fire again.
///
/// One detector instance tracks one stick. Feed it the *dead-zoned* stick `(x,
/// y)` each frame via [`update`](StickNavTracker::update) (the same radial dead
/// zone gameplay movement uses, so a stick already at rest reads `(0, 0)`).
///
/// Diagonal handling: the dominant axis wins, so a diagonal push produces a
/// single directional intent rather than two. While the stick stays past the
/// dead zone, no further intents fire — repeat-on-hold is the focus engine's
/// dt-clocked timer (Task 3), not an input-edge concern.
#[derive(Debug, Default)]
pub struct StickNavTracker {
    /// The direction the stick is currently latched in, or `None` when it sits
    /// inside the dead zone. A new intent fires only on a `None → Some` or a
    /// `Some(a) → Some(b)` direction change.
    latched: Option<NavIntent>,
}

impl StickNavTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the current dead-zoned stick position and return a nav intent on a
    /// fresh crossing, or `None` when the position is inside the dead zone or
    /// the stick is still held in its already-latched direction.
    ///
    /// `(x, y)` follows the gameplay stick convention: `+y` is up, `+x` is
    /// right. A stick at rest (inside the dead zone) reads `(0, 0)` and clears
    /// the latch so the next push fires again.
    pub fn update(&mut self, x: f32, y: f32) -> Option<NavIntent> {
        let direction = Self::direction(x, y);

        match direction {
            None => {
                // Returned to the dead zone — re-arm for the next push.
                self.latched = None;
                None
            }
            Some(dir) if self.latched == Some(dir) => {
                // Still held the same way; the hold-to-repeat timer (Task 3)
                // owns any subsequent firing, not this edge detector.
                None
            }
            Some(dir) => {
                // Fresh crossing or a direction change: latch and emit once.
                self.latched = Some(dir);
                Some(dir)
            }
        }
    }

    /// Resolve a dead-zoned stick position to a cardinal nav direction, or
    /// `None` when it sits inside the dead zone (both axes zero). The
    /// larger-magnitude axis wins so a diagonal produces a single direction.
    fn direction(x: f32, y: f32) -> Option<NavIntent> {
        if x == 0.0 && y == 0.0 {
            return None;
        }
        if x.abs() >= y.abs() {
            Some(if x > 0.0 {
                NavIntent::Right
            } else {
                NavIntent::Left
            })
        } else {
            Some(if y > 0.0 {
                NavIntent::Up
            } else {
                NavIntent::Down
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_cover_full_vocabulary() {
        // Pins the wire-name spelling that the TS/Luau union and JSON authors
        // depend on. A rename here is a wire break.
        let pairs = [
            (NavIntent::Up, "nav.up"),
            (NavIntent::Down, "nav.down"),
            (NavIntent::Left, "nav.left"),
            (NavIntent::Right, "nav.right"),
            (NavIntent::Next, "nav.next"),
            (NavIntent::Prev, "nav.prev"),
            (NavIntent::Confirm, "nav.confirm"),
            (NavIntent::Cancel, "nav.cancel"),
            (NavIntent::Menu, "nav.menu"),
            (NavIntent::Options, "nav.options"),
        ];
        for (intent, name) in pairs {
            assert_eq!(intent.wire_name(), name);
        }
    }

    // --- Keyboard mapping ---

    #[test]
    fn arrow_keys_map_to_directional_nav() {
        assert_eq!(
            nav_intent_for_key(KeyCode::ArrowUp, false),
            Some(NavIntent::Up)
        );
        assert_eq!(
            nav_intent_for_key(KeyCode::ArrowDown, false),
            Some(NavIntent::Down)
        );
        assert_eq!(
            nav_intent_for_key(KeyCode::ArrowLeft, false),
            Some(NavIntent::Left)
        );
        assert_eq!(
            nav_intent_for_key(KeyCode::ArrowRight, false),
            Some(NavIntent::Right)
        );
    }

    #[test]
    fn enter_maps_to_confirm() {
        assert_eq!(
            nav_intent_for_key(KeyCode::Enter, false),
            Some(NavIntent::Confirm)
        );
        assert_eq!(
            nav_intent_for_key(KeyCode::NumpadEnter, false),
            Some(NavIntent::Confirm)
        );
    }

    #[test]
    fn escape_from_gameplay_is_menu_and_inside_ui_is_cancel() {
        // The capturing-tree flag is the only difference: no capturing tree
        // (gameplay) opens the menu; a capturing tree backs out.
        assert_eq!(
            nav_intent_for_key(KeyCode::Escape, false),
            Some(NavIntent::Menu),
            "Escape from gameplay opens the menu",
        );
        assert_eq!(
            nav_intent_for_key(KeyCode::Escape, true),
            Some(NavIntent::Cancel),
            "Escape inside a capturing tree cancels",
        );
    }

    #[test]
    fn non_nav_keys_map_to_none() {
        assert_eq!(nav_intent_for_key(KeyCode::KeyW, false), None);
        assert_eq!(nav_intent_for_key(KeyCode::Space, false), None);
    }

    // --- Gamepad mapping ---

    #[test]
    fn dpad_maps_to_directional_nav() {
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::DPadUp),
            Some(NavIntent::Up)
        );
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::DPadDown),
            Some(NavIntent::Down)
        );
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::DPadLeft),
            Some(NavIntent::Left)
        );
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::DPadRight),
            Some(NavIntent::Right)
        );
    }

    #[test]
    fn face_and_system_buttons_map_per_bindings() {
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::South),
            Some(NavIntent::Confirm)
        );
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::East),
            Some(NavIntent::Cancel)
        );
        // nav.menu = Start; nav.options = Select/Back.
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::Start),
            Some(NavIntent::Menu)
        );
        assert_eq!(
            nav_intent_for_gamepad_button(GilrsButton::Select),
            Some(NavIntent::Options)
        );
    }

    #[test]
    fn unmapped_gamepad_button_is_none() {
        assert_eq!(nav_intent_for_gamepad_button(GilrsButton::North), None);
    }

    // --- Stick edge detection ---

    #[test]
    fn stick_past_deadzone_produces_exactly_one_intent_per_crossing() {
        let mut tracker = StickNavTracker::new();

        // First push past the dead zone fires once.
        assert_eq!(tracker.update(0.0, 0.8), Some(NavIntent::Up));
        // Holding it produces no further intents — repeat is Task 3's timer.
        assert_eq!(tracker.update(0.0, 0.9), None);
        assert_eq!(tracker.update(0.0, 0.8), None);

        // Return inside the dead zone, then push again — fires once more.
        assert_eq!(tracker.update(0.0, 0.0), None);
        assert_eq!(tracker.update(0.0, 0.7), Some(NavIntent::Up));
    }

    #[test]
    fn stick_direction_change_fires_new_intent_without_recentering() {
        let mut tracker = StickNavTracker::new();
        assert_eq!(tracker.update(0.8, 0.0), Some(NavIntent::Right));
        // Flicking straight to the opposite direction is a fresh crossing.
        assert_eq!(tracker.update(-0.8, 0.0), Some(NavIntent::Left));
    }

    #[test]
    fn stick_dominant_axis_wins_on_diagonal() {
        let mut tracker = StickNavTracker::new();
        // x dominates → Right, a single intent rather than Right + Up.
        assert_eq!(tracker.update(0.7, 0.3), Some(NavIntent::Right));
        tracker.update(0.0, 0.0);
        // y dominates → Down.
        assert_eq!(tracker.update(0.3, -0.7), Some(NavIntent::Down));
    }

    #[test]
    fn stick_inside_deadzone_never_fires() {
        let mut tracker = StickNavTracker::new();
        // A dead-zoned stick reads (0, 0); no intent.
        assert_eq!(tracker.update(0.0, 0.0), None);
    }
}
