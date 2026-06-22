// Diagnostic input channel: modifier chords for engine debug actions.
// See: context/lib/input.md §7

use winit::keyboard::KeyCode;

/// Modifier-key state at the moment a key event is processed.
///
/// Left and right modifiers are equivalent: ShiftLeft and ShiftRight both
/// set `shift`. Chord matching uses `==`, so an extra modifier suppresses
/// the chord — preventing OS shortcuts and editor binds from accidentally
/// firing diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub super_key: bool,
}

impl Modifiers {
    pub const ALT_SHIFT: Modifiers = Modifiers {
        shift: true,
        alt: true,
        ctrl: false,
        super_key: false,
    };
}

/// Engine-side debug actions invoked by diagnostic chords.
///
/// Distinct from `Action`: diagnostic actions are consumed by the engine
/// itself (renderer, visibility stats), never by game logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticAction {
    /// Toggle the cull-status all-leaves x-ray wireframe overlay on/off. Shows
    /// all loaded world triangles color-coded by GPU cull status: green =
    /// rendered, red = explicitly frustum-culled, cyan = not submitted by the
    /// GPU cull pass, including skipped subtree descendants and leaves outside
    /// CPU-visible cells.
    ToggleWireframe,
    /// Dump the next frame's portal walk (visited leaves, rejected portals,
    /// reject reasons) to the log. One-shot per press.
    DumpPortalWalk,
    /// Flip the surface present mode between vsync on (`AutoVsync`) and
    /// vsync off (`AutoNoVsync`). Used to compare vsync-pinned frametimes
    /// against real CPU cost when the meter is saturated against the frame
    /// budget.
    ToggleVsync,
    /// Play the test SFX tone (`sfx/test_tone`) on the SFX bus. Bound to
    /// `Alt+Shift+P`. A real-device audio smoke check: confirms a played sound
    /// reaches the OS. Needs a level loaded so the sound registry is populated;
    /// if the fixture isn't present `Audio::play` warns gracefully.
    PlayTestSfx,
    /// Toggle the egui debug panel visibility. Bound to `Alt+Shift+Backquote`.
    /// Fires through `App::handle_diagnostic_action`, which also shifts
    /// `InputFocus` between `DevTools` and `Gameplay`. Feature-gated:
    /// without `dev-tools`, this variant and its chord are absent.
    #[cfg(feature = "dev-tools")]
    ToggleDebugPanel,
    /// Toggle the in-world navmesh overlay (region rectangles + portal edges).
    /// Bound to `Alt+Shift+N`. Drawn through the renderer's debug-line pass from
    /// the loaded `NavMeshSection`. Feature-gated: without `dev-tools`, this
    /// variant and its chord are absent.
    #[cfg(feature = "dev-tools")]
    ToggleNavOverlay,
    /// Exercise the runtime level lifecycle by unloading the current level and
    /// loading the second dev map. Bound to `Alt+Shift+L`. Feature-gated:
    /// without `dev-tools`, this variant and its chord are absent.
    #[cfg(feature = "dev-tools")]
    CycleDevLevel,
    /// Spawn (once) a navmesh chase agent that chases the player pawn — or the
    /// camera when no pawn exists — re-targeting every tick. Bound to
    /// `Alt+Shift+G`. The "chase me" pathfinding demo. Feature-gated: without
    /// `dev-tools`, this variant and its chord are absent.
    #[cfg(feature = "dev-tools")]
    SpawnChaseAgent,
}

/// A modifier+key combination bound to a diagnostic action.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticChord {
    pub modifiers: Modifiers,
    pub key: KeyCode,
    pub action: DiagnosticAction,
}

/// Resolves keyboard events into diagnostic actions.
///
/// Tracks modifier state across events and matches incoming key presses
/// against the registered chord table. Repeats are suppressed so each
/// press fires its action exactly once.
pub struct DiagnosticInputs {
    chords: Vec<DiagnosticChord>,
    modifier_state: Modifiers,
}

impl DiagnosticInputs {
    pub fn new(chords: Vec<DiagnosticChord>) -> Self {
        Self {
            chords,
            modifier_state: Modifiers::default(),
        }
    }

    /// Process a keyboard event. Updates modifier state from Shift/Alt/Ctrl/
    /// Super key events, then returns the diagnostic action — if any — that
    /// this event triggers.
    ///
    /// Returns `None` for releases, repeats, modifier-only events, and
    /// chords that don't match the current modifier state exactly.
    pub fn handle_key(
        &mut self,
        code: KeyCode,
        pressed: bool,
        repeat: bool,
    ) -> Option<DiagnosticAction> {
        match code {
            KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                self.modifier_state.shift = pressed;
                return None;
            }
            KeyCode::AltLeft | KeyCode::AltRight => {
                self.modifier_state.alt = pressed;
                return None;
            }
            KeyCode::ControlLeft | KeyCode::ControlRight => {
                self.modifier_state.ctrl = pressed;
                return None;
            }
            KeyCode::SuperLeft | KeyCode::SuperRight => {
                self.modifier_state.super_key = pressed;
                return None;
            }
            _ => {}
        }

        if !pressed || repeat {
            return None;
        }

        self.chords
            .iter()
            .find(|c| c.key == code && c.modifiers == self.modifier_state)
            .map(|c| c.action)
    }

    /// Whether Shift is currently held (left or right — they're equivalent).
    /// Read by the App's Escape routing: `Shift+Esc` is the dev quit chord,
    /// plain `Esc` falls through to the menu/cancel path. Reuses the same
    /// modifier tracking the diagnostic chords drive.
    pub fn shift_held(&self) -> bool {
        self.modifier_state.shift
    }

    /// Reset all modifier state. Called when the window loses focus so a
    /// modifier released off-window doesn't leave the resolver thinking it's
    /// still held.
    pub fn clear_modifiers(&mut self) {
        self.modifier_state = Modifiers::default();
    }
}

/// Default diagnostic chord table. All chords live in the `Alt+Shift+`
/// namespace per `context/lib/input.md` §7.
pub fn default_diagnostic_chords() -> Vec<DiagnosticChord> {
    vec![
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::Backslash,
            action: DiagnosticAction::ToggleWireframe,
        },
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::Digit1,
            action: DiagnosticAction::DumpPortalWalk,
        },
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::KeyV,
            action: DiagnosticAction::ToggleVsync,
        },
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::KeyP,
            action: DiagnosticAction::PlayTestSfx,
        },
        #[cfg(feature = "dev-tools")]
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::Backquote,
            action: DiagnosticAction::ToggleDebugPanel,
        },
        #[cfg(feature = "dev-tools")]
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::KeyN,
            action: DiagnosticAction::ToggleNavOverlay,
        },
        #[cfg(feature = "dev-tools")]
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::KeyL,
            action: DiagnosticAction::CycleDevLevel,
        },
        #[cfg(feature = "dev-tools")]
        DiagnosticChord {
            modifiers: Modifiers::ALT_SHIFT,
            key: KeyCode::KeyG,
            action: DiagnosticAction::SpawnChaseAgent,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> DiagnosticInputs {
        DiagnosticInputs::new(default_diagnostic_chords())
    }

    // --- Modifier state tracking ---

    #[test]
    fn shift_press_sets_shift_modifier() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        assert!(d.modifier_state.shift);
    }

    #[test]
    fn shift_release_clears_shift_modifier() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::ShiftLeft, false, false);
        assert!(!d.modifier_state.shift);
    }

    #[test]
    fn left_and_right_shift_are_equivalent() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftRight, true, false);
        d.handle_key(KeyCode::AltRight, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, Some(DiagnosticAction::ToggleWireframe));
    }

    #[test]
    fn modifier_keys_alone_never_fire_an_action() {
        let mut d = fresh();
        let action = d.handle_key(KeyCode::ShiftLeft, true, false);
        assert_eq!(action, None);
    }

    // --- Chord matching ---

    #[test]
    fn alt_shift_backslash_fires_toggle_wireframe() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, Some(DiagnosticAction::ToggleWireframe));
    }

    #[test]
    fn alt_shift_digit1_fires_dump_portal_walk() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Digit1, true, false);
        assert_eq!(action, Some(DiagnosticAction::DumpPortalWalk));
    }

    #[test]
    fn alt_shift_v_fires_toggle_vsync() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::KeyV, true, false);
        assert_eq!(action, Some(DiagnosticAction::ToggleVsync));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn alt_shift_backquote_fires_toggle_debug_panel() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backquote, true, false);
        assert_eq!(action, Some(DiagnosticAction::ToggleDebugPanel));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn alt_shift_n_fires_toggle_nav_overlay() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::KeyN, true, false);
        assert_eq!(action, Some(DiagnosticAction::ToggleNavOverlay));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn alt_shift_l_fires_cycle_dev_level() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::KeyL, true, false);
        assert_eq!(action, Some(DiagnosticAction::CycleDevLevel));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn alt_shift_g_fires_spawn_chase_agent() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::KeyG, true, false);
        assert_eq!(action, Some(DiagnosticAction::SpawnChaseAgent));
    }

    #[test]
    fn shift_alone_does_not_fire_alt_shift_chord() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, None);
    }

    #[test]
    fn alt_alone_does_not_fire_alt_shift_chord() {
        let mut d = fresh();
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, None);
    }

    #[test]
    fn extra_modifier_suppresses_chord() {
        // Alt+Shift+Ctrl+\\ should NOT fire ToggleWireframe — extra modifier.
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        d.handle_key(KeyCode::ControlLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, None);
    }

    #[test]
    fn cmd_modifier_suppresses_chord() {
        // Cmd+Alt+Shift+\\ on Mac is an OS-level chord; must not fire ours.
        let mut d = fresh();
        d.handle_key(KeyCode::SuperLeft, true, false);
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, None);
    }

    // --- Rising-edge semantics ---

    #[test]
    fn key_release_does_not_fire_action() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, false, false);
        assert_eq!(action, None);
    }

    #[test]
    fn key_repeat_does_not_fire_action() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        let action = d.handle_key(KeyCode::Backslash, true, true);
        assert_eq!(action, None);
    }

    #[test]
    fn second_press_after_release_fires_again() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);

        let first = d.handle_key(KeyCode::Backslash, true, false);
        d.handle_key(KeyCode::Backslash, false, false);
        let second = d.handle_key(KeyCode::Backslash, true, false);

        assert_eq!(first, Some(DiagnosticAction::ToggleWireframe));
        assert_eq!(second, Some(DiagnosticAction::ToggleWireframe));
    }

    // --- clear_modifiers ---

    #[test]
    fn clear_modifiers_resets_held_state() {
        let mut d = fresh();
        d.handle_key(KeyCode::ShiftLeft, true, false);
        d.handle_key(KeyCode::AltLeft, true, false);
        d.clear_modifiers();

        // Backslash alone should not fire ToggleWireframe after clear.
        let action = d.handle_key(KeyCode::Backslash, true, false);
        assert_eq!(action, None);
    }

    // --- Default chord table ---

    #[test]
    fn default_chords_use_only_alt_shift_namespace() {
        for chord in default_diagnostic_chords() {
            assert_eq!(
                chord.modifiers,
                Modifiers::ALT_SHIFT,
                "diagnostic chord {:?} escapes the Alt+Shift namespace",
                chord.action,
            );
        }
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn default_chords_include_toggle_debug_panel() {
        let has_toggle = default_diagnostic_chords()
            .into_iter()
            .any(|c| c.action == DiagnosticAction::ToggleDebugPanel);
        assert!(
            has_toggle,
            "ToggleDebugPanel must be present in default chord table when dev-tools is enabled"
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn toggle_debug_panel_chord_stays_alt_shift_backquote() {
        let chord = default_diagnostic_chords()
            .into_iter()
            .find(|c| c.action == DiagnosticAction::ToggleDebugPanel)
            .expect("ToggleDebugPanel chord must be registered");

        assert_eq!(chord.modifiers, Modifiers::ALT_SHIFT);
        assert_eq!(chord.key, KeyCode::Backquote);
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn default_dev_tool_chord_table_has_no_spatial_entry_point() {
        let mut count = 0;
        for chord in default_diagnostic_chords() {
            match chord.action {
                DiagnosticAction::ToggleWireframe => assert_eq!(chord.key, KeyCode::Backslash),
                DiagnosticAction::DumpPortalWalk => assert_eq!(chord.key, KeyCode::Digit1),
                DiagnosticAction::ToggleVsync => assert_eq!(chord.key, KeyCode::KeyV),
                DiagnosticAction::PlayTestSfx => assert_eq!(chord.key, KeyCode::KeyP),
                DiagnosticAction::ToggleDebugPanel => assert_eq!(chord.key, KeyCode::Backquote),
                DiagnosticAction::ToggleNavOverlay => assert_eq!(chord.key, KeyCode::KeyN),
                DiagnosticAction::CycleDevLevel => assert_eq!(chord.key, KeyCode::KeyL),
                DiagnosticAction::SpawnChaseAgent => assert_eq!(chord.key, KeyCode::KeyG),
            }
            count += 1;
        }

        assert_eq!(
            count, 8,
            "Spatial diagnostics must stay inside the existing diagnostics panel entry point",
        );
    }

    #[test]
    fn default_chords_have_no_duplicate_keys() {
        let chords = default_diagnostic_chords();
        for (i, a) in chords.iter().enumerate() {
            for b in chords.iter().skip(i + 1) {
                assert!(
                    !(a.key == b.key && a.modifiers == b.modifiers),
                    "duplicate chord: {:?} and {:?} both bind {:?}+{:?}",
                    a.action,
                    b.action,
                    a.modifiers,
                    a.key,
                );
            }
        }
    }
}
