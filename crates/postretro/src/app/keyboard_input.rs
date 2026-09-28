// Keyboard intake: diagnostic chords, the UI-dispatch seam (nav intents and
// text entry), and the gameplay forward.
// See: context/lib/input.md §5, §7

use crate::*;

impl App {
    /// Route one keyboard event. `egui_consumed` is the dev-tools overlay's
    /// consumption verdict for this event (always false without dev tools).
    pub(crate) fn handle_keyboard_input(&mut self, key_event: &KeyEvent, egui_consumed: bool) {
        if let PhysicalKey::Code(code) = key_event.physical_key {
            let pressed = key_event.state.is_pressed();

            // Modifier-only key events always feed the diagnostic
            // resolver — even when egui consumes them — so its
            // modifier tracking stays current and `Alt+Shift+Backquote`
            // remains resolvable while the panel has focus.
            let is_modifier_key = matches!(
                code,
                winit::keyboard::KeyCode::ShiftLeft
                    | winit::keyboard::KeyCode::ShiftRight
                    | winit::keyboard::KeyCode::AltLeft
                    | winit::keyboard::KeyCode::AltRight
                    | winit::keyboard::KeyCode::ControlLeft
                    | winit::keyboard::KeyCode::ControlRight
                    | winit::keyboard::KeyCode::SuperLeft
                    | winit::keyboard::KeyCode::SuperRight
            );

            if egui_consumed {
                // egui owns this event. Keep modifier tracking current
                // so the toggle chord still resolves once the panel is
                // open, but do not forward to the input system or fire
                // any other diagnostic chord.
                if is_modifier_key {
                    let _ = self
                        .diagnostic_inputs
                        .handle_key(code, pressed, key_event.repeat);
                }
                // The toggle chord (`Alt+Shift+Backquote`) is reachable
                // even when egui consumes the keypress — no egui widget
                // binds it, so a targeted check here is unambiguous.
                // See: context/lib/input.md §7
                #[cfg(feature = "dev-tools")]
                if !is_modifier_key {
                    if let Some(action) =
                        self.diagnostic_inputs
                            .handle_key(code, pressed, key_event.repeat)
                    {
                        if action == DiagnosticAction::ToggleDebugPanel {
                            self.handle_diagnostic_action(action);
                        }
                    }
                }
                return;
            }

            // Chord resolver runs first: owns Alt+Shift+ modifier
            // tracking and fires only on a clean rising edge.
            if let Some(action) = self
                .diagnostic_inputs
                .handle_key(code, pressed, key_event.repeat)
            {
                self.handle_diagnostic_action(action);
            }

            // UI-dispatch seam, ahead of the gameplay forward and
            // mirroring the `egui_consumed` gate: when the active UI
            // layer is in Capture mode the event is consumed (queued
            // for next-frame game logic) and NOT forwarded to the
            // action system this frame. `InputFocus::Menu` is the
            // intended structural home for this capture.
            //
            // Key-down edges resolve to a nav intent (arrows / enter /
            // escape / tab); the kinded payload rides the queue. Held
            // repeats and non-nav keys carry no intent (the seam still
            // suppresses the gameplay forward). Escape's menu-vs-cancel
            // split needs the "is a capturing tree on the stack?" flag,
            // sourced from the modal stack's top capture mode.
            // See: context/lib/input.md
            // The UI seam and gameplay forward are session-owned; boot
            // phase (pre-install) ignores gameplay/UI key input. The
            // diagnostic resolver above already ran so dev chords still
            // work during boot. Mode-signal / menu-toggle votes are
            // collected here and applied after the session borrow ends.
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let mut record_nav_signal = false;
            let mut set_menu_toggle = false;

            // A directional key RELEASE stops the focus engine's
            // hold-to-repeat (the press-edge queue carries no release, so
            // the focus ring's repeat clock is cleared here). Cancel never
            // repeats, so only directional keys matter for nav repeat.
            if !pressed
                && matches!(
                    code,
                    winit::keyboard::KeyCode::ArrowUp
                        | winit::keyboard::KeyCode::ArrowDown
                        | winit::keyboard::KeyCode::ArrowLeft
                        | winit::keyboard::KeyCode::ArrowRight
                )
            {
                session.ui_focus.release_repeat();
            }
            // A confirm key (Enter) RELEASE stops the activation-repeat clock
            // (M13 Text-Entry, Task 2): a held `repeatOnHold` button stops
            // re-firing once the confirm key is released, mirroring the
            // directional release above.
            if !pressed
                && matches!(
                    code,
                    winit::keyboard::KeyCode::Enter | winit::keyboard::KeyCode::NumpadEnter
                )
            {
                session.ui_focus.release_confirm_repeat();
            }
            // Text-entry routing (M13 Text-Entry, Task 3): while a text-entry
            // tree is the top of the modal stack, hardware key-down events
            // drive the edit surface instead of nav. The LOGICAL key resolves
            // Backspace/Enter/Escape first (so a `\u{8}` Backspace text or a
            // `\r` Enter text never leaks through the printable channel); only
            // a non-control printable `KeyEvent.text` becomes a `Text` intent.
            // Enter/Escape ride the queue as `nav.confirm`/`nav.cancel`, which
            // the focus-resolution stage intercepts for commit/cancel.
            let text_entry_open = session.modal_stack.active_text_entry_target().is_some();
            // Text entry intentionally honors OS key-repeat (Text-Entry AC4:
            // hardware-key repeat comes from the OS): a held Backspace/letter
            // appends/deletes on each auto-repeat. All OTHER UI input stays
            // edge-only (`!key_event.repeat`) — nav intents must not re-fire on
            // a held key, since the focus engine's own dt clock owns nav repeat.
            let nav_intent = if pressed && (!key_event.repeat || text_entry_open) {
                if text_entry_open {
                    // A key inside text entry is always a `focus`-mode signal.
                    record_nav_signal = true;
                    match input::text_entry_key(&key_event.logical_key, key_event.text.as_deref()) {
                        Some(input::TextEntryKey::Append(s)) => {
                            Some(input::UiIntentPayload::Text(s))
                        }
                        Some(input::TextEntryKey::Backspace) => {
                            Some(input::UiIntentPayload::Backspace)
                        }
                        Some(input::TextEntryKey::Commit) => {
                            Some(input::UiIntentPayload::Nav(input::NavIntent::Confirm))
                        }
                        Some(input::TextEntryKey::Cancel) => {
                            Some(input::UiIntentPayload::Nav(input::NavIntent::Cancel))
                        }
                        None => None,
                    }
                } else {
                    // Escape's menu-vs-cancel split: a capturing tree on the
                    // stack routes Escape to `nav.cancel`; from gameplay it
                    // opens the menu (`nav.menu`). The seam's `Capture` mode is
                    // set by `reconcile_ui_focus` from the modal stack's top
                    // capture mode, so it IS the "capturing tree present"
                    // predicate. See: context/lib/input.md §7
                    let capturing = session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
                    let intent = input::nav_intent_for_key(code, capturing);
                    if intent.is_some() {
                        // A nav key (arrows/enter/escape/tab) is a `focus`-mode
                        // signal — it switches the interaction mode off pointer.
                        record_nav_signal = true;
                    }
                    // Escape-from-gameplay maps to `nav.menu` (opens the pause
                    // menu). The seam is `Passthrough` from gameplay and queues
                    // nothing, so route the toggle through the punch-through flag.
                    if intent == Some(input::NavIntent::Menu) {
                        set_menu_toggle = true;
                    }
                    intent.map(input::UiIntentPayload::Nav)
                }
            } else {
                None
            };
            if session
                .ui_dispatch
                .dispatch_event(nav_intent)
                .forwards_to_gameplay()
                && session.input_focus == InputFocus::Gameplay
            {
                // Only Gameplay forwards keys to the action system. When
                // the debug panel (or future menu) owns focus, WASD must
                // not drive the camera even though egui leaves
                // `consumed = false` for non-text widgets like sliders.
                session.input_system.handle_keyboard_event(code, pressed);
            }

            if record_nav_signal {
                self.record_mode_signal(scripting_systems::input_mode::ModeSignal::NavInput);
            }
            if set_menu_toggle {
                self.pending_menu_toggle = true;
            }
        }
    }
}
