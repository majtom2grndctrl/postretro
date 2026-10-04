// UI activation routing: reserved `ui.*` actions, focused-button activation,
// slider nav capture, text-entry commit/cancel, and pause-menu policy.
// See: context/lib/ui.md §4

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiButtonAction {
    CommitTextEntry,
    CloseDialog,
    ExitToDesktop,
    QuitToMenu,
    NamedReaction,
}

pub(crate) fn classify_ui_button_action(on_press: &str) -> UiButtonAction {
    match on_press {
        postretro_ui::actions::COMMIT_TEXT_ENTRY_ACTION => UiButtonAction::CommitTextEntry,
        postretro_ui::actions::CLOSE_DIALOG_ACTION => UiButtonAction::CloseDialog,
        postretro_ui::actions::EXIT_TO_DESKTOP_ACTION => UiButtonAction::ExitToDesktop,
        postretro_ui::actions::QUIT_TO_MENU_ACTION => UiButtonAction::QuitToMenu,
        _ => UiButtonAction::NamedReaction,
    }
}

pub(crate) fn focused_button_on_press(
    rects: Option<&postretro_ui::tree::FocusRectList>,
    focused_id: Option<&str>,
) -> Option<String> {
    use postretro_ui::tree::NodeInteraction;

    let focused_id = focused_id?;
    rects?
        .rects
        .iter()
        .find(|r| r.id == focused_id)
        // A disabled focused node is non-interactive (M13 G2-T3): block its
        // activation regardless of how the focus arrived (a pre-existing focus
        // that became disabled, or a click that fell through). The focus engine
        // already keeps disabled nodes unreachable; this is the App-side gate on
        // the activation path itself.
        .filter(|r| !r.disabled)
        .and_then(|r| match &r.interaction {
            Some(NodeInteraction::Button { on_press, .. }) => Some(on_press.clone()),
            _ => None,
        })
}

pub(crate) fn route_ui_button_action(
    on_press: &str,
    modal_stack: &mut postretro_ui::modal_stack::ModalStack,
) -> UiButtonAction {
    match classify_ui_button_action(on_press) {
        UiButtonAction::CloseDialog => {
            modal_stack.pop();
            UiButtonAction::CloseDialog
        }
        other => other,
    }
}

pub(crate) fn apply_pause_menu_nav_policy(modal_stack: &mut postretro_ui::modal_stack::ModalStack) {
    match modal_stack.active_name() {
        Some(postretro_ui::demo::PAUSE_MENU_NAME) => modal_stack.pop(),
        None => modal_stack.push_named(postretro_ui::demo::PAUSE_MENU_NAME, None),
        Some(_) => {}
    }
}

/// Running `nav.cancel`: close the active `pauseMenu`, accessibility panel, or
/// engine display-mode confirmation (reverting its pending change); also close
/// a submenu pushed above the pause menu or frontend root. A submenu opened from
/// the pause menu (the options screen) returns to it. Other trees own their own
/// cancel policy.
/// `close_frontend_submenu` is the frontend's verdict: its root is pushed and is
/// not on top.
pub(crate) fn apply_running_cancel_policy(
    modal_stack: &mut postretro_ui::modal_stack::ModalStack,
    close_frontend_submenu: bool,
) {
    let active = modal_stack.active_name();
    let pause_submenu = active != Some(postretro_ui::demo::PAUSE_MENU_NAME)
        && modal_stack.contains_pushed(postretro_ui::demo::PAUSE_MENU_NAME);
    if active == Some(postretro_ui::demo::PAUSE_MENU_NAME)
        || active == Some(postretro_ui::demo::DISPLAY_MODE_CONFIRM_NAME)
        || active == Some(postretro_ui::demo::ACCESSIBILITY_PANEL_NAME)
        || pause_submenu
        || close_frontend_submenu
    {
        modal_stack.pop();
    }
}

impl App {
    /// Apply slider nav-capture for the focused slider (M13 Goal F, Task 4).
    ///
    /// The currently focused node
    /// (last frame's `ui_focused_id`, the focus going into this frame) is matched
    /// against the exported focus rects; if it is a `slider`, each nav intent whose
    /// wire name is in the slider's `captures_nav` is REMOVED from `nav_intents`
    /// (the focus engine never sees it) and, when directional, steps the bound value
    /// by `step` clamped to the slider's min/max, enqueuing a `setState` write
    /// applied at the game-logic command drain (the bound slot changes on N+1).
    pub(crate) fn apply_slider_nav_capture(&mut self, nav_intents: &mut Vec<input::NavIntent>) {
        use postretro_ui::tree::NodeInteraction;

        let Some(focused_id) = self.ui_focused_id.as_deref() else {
            return;
        };
        let Some(rects) = self
            .session
            .as_ref()
            .and_then(|session| session.ui_focus_rects.as_ref())
        else {
            return;
        };
        // Resolve the focused slider's interaction + its bound slot (clone out so
        // the immutable borrow of the rect list drops before the slot/queue work).
        let slider = rects
            .rects
            .iter()
            .find(|r| r.id == focused_id)
            .and_then(|r| match &r.interaction {
                Some(interaction @ NodeInteraction::Slider { slot, min, .. }) => {
                    Some((interaction.clone(), slot.clone(), *min))
                }
                _ => None,
            });
        let Some((interaction, slot, min)) = slider else {
            return;
        };
        let owner = rects.owner.clone();

        let script_ctx = self
            .session
            .as_ref()
            .expect("frontend session installed")
            .scripting
            .script_ctx
            .clone();
        // The slider's current value: its bound slot reading, or `min` as a floor
        // when the slot is unset or non-numeric (a sane starting point).
        let current = {
            let table = script_ctx.slot_table.borrow();
            match table.get(&slot).and_then(|r| r.value.as_ref()) {
                Some(postretro_entities::SlotValue::Number(n)) => *n,
                _ => min,
            }
        };

        // Peel off captured nav intents (mutating `nav_intents`) and compute the
        // stepped value; emit one `setState` for the new clamped value.
        if let Some(next) = input::capture_slider_step(&interaction, current, nav_intents) {
            // An engine-tier slider on a readonly `accessibility.*` slot steps its
            // field through the panel's field action instead of `setState`.
            if self.route_engine_accessibility_slider(&slot, owner.as_ref(), current, next) {
                return;
            }
            script_ctx
                .system_commands
                .push(SystemReactionCommand::SetState {
                    slot,
                    value: serde_json::json!(next),
                    dispatch_source: "ui.slider".to_string(),
                    dispatch_values: Vec::new(),
                });
        }
    }

    /// Fire a focused button's `onPress` on activation. Reserved `ui.*` actions
    /// are handled App-side before ordinary names fall through to the shared
    /// named-reaction path, so gamepad confirm and pointer click produce the same
    /// observable effect.
    pub(crate) fn fire_focused_button_activation(&mut self, focused_id: Option<&str>) {
        self.fire_focused_button_activation_with_display_mode(focused_id, |app, action| {
            app.apply_display_mode_action(action);
        });
    }

    /// Keep activation routing shared with adapters that supply a window backend.
    pub(crate) fn fire_focused_button_activation_with_display_mode(
        &mut self,
        focused_id: Option<&str>,
        apply_display_mode: impl FnOnce(&mut Self, postretro_ui::actions::DisplayModeAction),
    ) {
        let on_press = focused_button_on_press(
            self.session
                .as_ref()
                .and_then(|session| session.ui_focus_rects.as_ref()),
            focused_id,
        );
        if let Some(on_press) = on_press {
            if let Some(action) = postretro_ui::actions::parse_display_mode_action(&on_press) {
                apply_display_mode(self, action);
                return;
            }
            if on_press == postretro_ui::actions::CLOSE_DIALOG_ACTION
                && self.display_mode_confirm_is_top()
            {
                apply_display_mode(self, postretro_ui::actions::DisplayModeAction::Revert);
                return;
            }
            if on_press == postretro_ui::actions::OPEN_ACCESSIBILITY_ACTION {
                self.open_accessibility_panel();
                return;
            }
            if let Some(action) = postretro_ui::actions::parse_accessibility_field_action(&on_press)
            {
                self.apply_accessibility_field_action(action.op, action.field);
                return;
            }
            let action = match self.session.as_mut() {
                Some(session) => route_ui_button_action(&on_press, &mut session.modal_stack),
                None => return,
            };
            match action {
                UiButtonAction::CommitTextEntry => self.commit_text_entry(),
                UiButtonAction::CloseDialog => {}
                UiButtonAction::ExitToDesktop => self.pending_exit_to_desktop = true,
                UiButtonAction::QuitToMenu => self.return_to_frontend(),
                UiButtonAction::NamedReaction => {
                    if let Some(session) = self.session.as_ref() {
                        let script_ctx = &session.scripting.script_ctx;
                        // Capture chained names (a `fire` step's target or a fired
                        // `Primitive`'s `on_complete`) and dispatch them, rather
                        // than discarding as before. A `wait` step enrolls its tail
                        // ahead of the tick loop; the frame-counter stamp keeps it
                        // from advancing in this same redraw.
                        let chained = fire_named_event_with_sequences(
                            &on_press,
                            &script_ctx.data_registry.borrow(),
                            &session.scripting.sequence_registry,
                            &session.scripting.reaction_registry,
                            &session.scripting.system_registry,
                            script_ctx,
                            None,
                        );
                        if !chained.is_empty() {
                            dispatch_deferred_named_events_with_sequences(
                                chained,
                                &script_ctx.data_registry.borrow(),
                                &session.scripting.sequence_registry,
                                &session.scripting.reaction_registry,
                                &session.scripting.system_registry,
                                script_ctx,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Resolve drained UI intents against the open text-entry surface (M13
    /// Text-Entry, Task 3). Returns `true` when a `nav.confirm` (commit) or
    /// `nav.cancel` (cancel) was consumed by text entry this frame, so the caller
    /// filters those intents out of the focus engine and skips the pause-menu path.
    ///
    /// No-op (returns `false`) when text entry is closed — the top tree declares no
    /// `text_entry_target`. While open:
    /// - `Text(s)` → an `AppendText { slot, text: s }` edit against the target slot,
    /// - `Backspace` → a `BackspaceText { slot }` edit against the target slot,
    /// - `nav.confirm` → commit: fire the opener's `on_commit`, then `PopTree`,
    /// - `nav.cancel` → cancel: `PopTree` only (edits stay in the slot; the opener
    ///   simply does not act on them — no rollback).
    ///
    /// Edits ride Task 1's text-edit command path (pushed onto the system-command
    /// queue, drained at `dispatch_system_commands`), so they land on the bound slot
    /// on the N+1 frame — the system's defining N→N+1 ordering. Commit and cancel act
    /// on the stack immediately at this game-logic phase; the seam reconciles next.
    pub(crate) fn resolve_text_entry_intents(&mut self, ui_intents: &[input::UiIntent]) -> bool {
        let Some(target) = self.session.as_ref().and_then(|session| {
            session
                .modal_stack
                .active_text_entry_target()
                .map(str::to_string)
        }) else {
            return false;
        };

        // Thread the currently-focused node's interaction (last frame's exported
        // focus, the focus going into this frame — same source `apply_slider_nav_capture`
        // reads) so `resolve_text_entry` can distinguish a confirm that lands on an
        // on-screen keyboard key from a keyboardless hardware Enter. A confirm on a
        // focusable button must flow to the focus engine (Task 4 fires the key's
        // `on_press` — `kbAppend_*` to type, or `done`'s commit sentinel); only a
        // confirm NOT on a button commits here. Without this the confirm was consumed
        // as Commit before the focus engine ran and the keyboard closed instead of
        // typing.
        let confirm_on_button = self.focused_node_is_activatable_button();

        // Pure resolution: drained intents → ordered edits + a terminal disposition.
        let resolution = input::resolve_text_entry(ui_intents, confirm_on_button);

        // Apply the edits through Task 1's text-edit command path (the bound slot
        // changes on the N+1 frame). Edits are queued before commit/cancel acts so a
        // committing reaction observes the slot as last edited.
        for edit in &resolution.edits {
            let command = match edit {
                input::TextEntryEdit::Append(text) => SystemReactionCommand::AppendText {
                    slot: target.clone(),
                    text: text.clone(),
                },
                input::TextEntryEdit::Backspace => SystemReactionCommand::BackspaceText {
                    slot: target.clone(),
                },
            };
            if let Some(session) = self.session.as_ref() {
                session.scripting.script_ctx.system_commands.push(command);
            }
        }

        match resolution.disposition {
            input::TextEntryDisposition::Commit => self.commit_text_entry(),
            input::TextEntryDisposition::Cancel => self.cancel_text_entry(),
            input::TextEntryDisposition::Open => {}
        }
        resolution.consumed_commit_or_cancel()
    }

    /// Whether the currently-focused node (last frame's `ui_focused_id` on the
    /// exported rect list) is an activatable `button`. The on-screen keyboard's
    /// keys are buttons, so this is the predicate `resolve_text_entry_intents` uses
    /// to keep a `nav.confirm` flowing to the focus engine (the key activates)
    /// rather than consuming it as a text-entry commit. Reads the same
    /// `ui_focused_id` + `ui_focus_rects` pair `apply_slider_nav_capture` does.
    pub(crate) fn focused_node_is_activatable_button(&self) -> bool {
        use postretro_ui::tree::NodeInteraction;
        let Some(focused_id) = self.ui_focused_id.as_deref() else {
            return false;
        };
        let Some(rects) = self
            .session
            .as_ref()
            .and_then(|session| session.ui_focus_rects.as_ref())
        else {
            return false;
        };
        rects
            .rects
            .iter()
            .find(|r| r.id == focused_id)
            .is_some_and(|r| matches!(r.interaction, Some(NodeInteraction::Button { .. })))
    }

    /// Commit the open text-entry surface (M13 Text-Entry, Task 3): fire the top
    /// tree's carried `on_commit` reaction (from the `PushTree` that opened it),
    /// THEN pop the tree. This is the shared commit seam — the hardware Enter key
    /// routes here, and Task 4's on-screen `done` button activation calls this same
    /// method so commit is not keyboard-only. A no-op when no tree is open.
    ///
    /// The `on_commit` reaction reads the bound slot's value (the entered text); the
    /// reaction fires synchronously here so it observes the slot as last edited.
    pub(crate) fn commit_text_entry(&mut self) {
        let on_commit = self
            .session
            .as_ref()
            .and_then(|session| session.modal_stack.active_on_commit().map(str::to_string));
        if let Some(on_commit) = on_commit
            && let Some(session) = self.session.as_ref()
        {
            let script_ctx = &session.scripting.script_ctx;
            let chained = fire_named_event_with_sequences(
                &on_commit,
                &script_ctx.data_registry.borrow(),
                &session.scripting.sequence_registry,
                &session.scripting.reaction_registry,
                &session.scripting.system_registry,
                script_ctx,
                None,
            );
            if !chained.is_empty() {
                dispatch_deferred_named_events_with_sequences(
                    chained,
                    &script_ctx.data_registry.borrow(),
                    &session.scripting.sequence_registry,
                    &session.scripting.reaction_registry,
                    &session.scripting.system_registry,
                    script_ctx,
                );
            }
        }
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.pop();
        }
    }

    /// Cancel the open text-entry surface (M13 Text-Entry, Task 3): pop the tree
    /// WITHOUT firing `on_commit`. Edits already applied to the bound slot are
    /// discarded simply by the opener not acting on them — there is no rollback.
    pub(crate) fn cancel_text_entry(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.pop();
        }
    }

    /// Apply the `nav.menu` pause-menu policy: pop the pause menu if it is active,
    /// open it when the modal stack is empty, and ignore the action while another
    /// modal is active. Wired to gamepad Start / Escape-from-gameplay through
    /// `pending_menu_toggle`. The capture-mode + cursor effect follows on the next
    /// `reconcile_ui_focus` (this game-logic phase).
    pub(crate) fn toggle_pause_menu(&mut self) {
        if let Some(session) = self.session.as_mut() {
            apply_pause_menu_nav_policy(&mut session.modal_stack);
        }
    }

    /// Reconcile the input-dispatch seam and coarse focus with the modal stack's
    /// top capture mode. Called in the game-logic phase after the system-command
    /// drains settle the stack, so the decision is in force for the NEXT frame's
    /// Input stage (the N→N+1 ordering the seam guarantees: a UI event consumed on
    /// frame N reaches game logic no earlier than N+1, and the capture/cursor side
    /// flips here, one game-logic phase before that read).
    ///
    /// - A capturing top tree drives `UiCaptureMode::Capture` (the seam queues
    ///   events for next-frame game logic instead of forwarding to gameplay) and
    ///   `InputFocus::Menu` (cursor released, player controls gated).
    /// - An empty or passthrough top hands input back: `Passthrough` at the seam,
    ///   and focus returns to `Gameplay` if it was `Menu`.
    ///
    /// While a capturing tree is up (Menu focus), the OS cursor's VISIBILITY then
    /// follows the interaction mode (M13 Goal F, Task 5): `pointer` shows it,
    /// `focus` hides it. This is inert when no capturing tree is up — gameplay
    /// owns the cursor (locked + hidden) and dev-tools owns its own.
    ///
    /// DevTools owns focus while the debug panel is open (it released the cursor
    /// and set `DevTools`); this reconcile never overrides that — the modal stack
    /// is gameplay UI, and the debug overlay is a separate, dev-only consumer.
    pub(crate) fn reconcile_ui_focus(&mut self) {
        // Read the session-owned inputs up front, then drop the borrow before
        // `set_input_focus` (which re-borrows the session). No-op before install.
        let (mode, current_focus) = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let mode = session.modal_stack.top_capture_mode();
            session.ui_dispatch.set_mode(mode.into());
            (mode, session.input_focus)
        };

        // The debug overlay owns focus while open — don't fight it.
        if current_focus == InputFocus::DevTools {
            return;
        }

        let want_menu = matches!(mode, postretro_ui::descriptor::CaptureMode::Capture);
        match (want_menu, current_focus) {
            // A capturing tree opened (or stayed open): enter Menu, release cursor.
            (true, InputFocus::Gameplay) => self.set_input_focus(InputFocus::Menu),
            // The capturing tree(s) closed: hand the cursor back to gameplay.
            (false, InputFocus::Menu) => self.set_input_focus(InputFocus::Gameplay),
            // Already in the right focus for the current capture mode.
            _ => {}
        }

        // Cursor visibility follows the interaction mode WHILE a capturing tree
        // is up. `set_input_focus(Menu)` released the cursor (visible) above; in
        // `focus` mode we additionally hide it so directional nav isn't cluttered
        // by a stray pointer. Mode is inert otherwise (no capturing tree).
        let cursor_visible = self
            .session
            .as_ref()
            .map(|session| (session.input_focus, session.ui_input_mode.cursor_visible()));
        if let Some((InputFocus::Menu, visible)) = cursor_visible
            && want_menu
            && let Some(ws) = self.window_state.as_ref()
        {
            ws.window.set_cursor_visible(visible);
        }
    }
}
