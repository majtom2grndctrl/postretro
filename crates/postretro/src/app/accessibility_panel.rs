// Engine accessibility panel: `ui.openAccessibility`, the reserved field-action
// family, flash-limiter attribution, engine-routed sliders, and the close-time
// save and first-launch record.
// See: context/lib/ui.md §4.1 · context/lib/player_options.md §5

use postretro_ui::demo::ACCESSIBILITY_PANEL_NAME;
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::tree::FocusRectOwner;

use crate::*;

impl App {
    /// `ui.openAccessibility`: push the engine panel over whatever shows. A
    /// panel already on the stack is not pushed twice.
    pub(crate) fn open_accessibility_panel(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session
            .modal_stack
            .contains_pushed(ACCESSIBILITY_PANEL_NAME)
        {
            session
                .modal_stack
                .push_named(ACCESSIBILITY_PANEL_NAME, None);
        }
    }

    /// Whether the press that produced this activation resolved against the
    /// engine panel's own export while the panel is the active tree. The stack's
    /// top at activation alone cannot attribute a press: the export is last
    /// frame's, and a same-frame pop can leave the panel on top of a press that
    /// hit another tree.
    fn press_is_from_active_panel(&self, owner: Option<&FocusRectOwner>) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let owned_by_panel = owner.is_some_and(|owner| {
            owner.name == ACCESSIBILITY_PANEL_NAME && owner.tier == ScopeTier::Engine
        });
        owned_by_panel
            && session.modal_stack.active_name() == Some(ACCESSIBILITY_PANEL_NAME)
            && session.modal_stack.active_tier() == Some(ScopeTier::Engine)
    }

    /// A button fired `ui.accessibility.<op>.<field>`. Writes the store and
    /// schedules the settled save; the options bridge projects the change into
    /// `accessibility.*` and reseeds the working copy later this frame. Any tree
    /// may fire a field action except the flash limiter's, which only the
    /// engine panel's own control honors.
    pub(crate) fn fire_accessibility_field_action(
        &mut self,
        op: &str,
        field: &str,
        owner: Option<&FocusRectOwner>,
    ) {
        if field == options::FLASH_LIMITER_FIELD && !self.press_is_from_active_panel(owner) {
            let from = owner.map_or("<none>", |owner| owner.name.as_str());
            log::warn!(
                "[UI] ignoring flash-limiter action from tree '{from}': only the engine accessibility panel changes the flash limiter"
            );
            return;
        }
        self.apply_accessibility_field_action(op, field);
    }

    fn apply_accessibility_field_action(&mut self, op: &str, field: &str) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match options::apply_panel_action(&mut session.player_options, field, op) {
            options::PanelActionOutcome::Written { .. } => {
                session
                    .options_bridge
                    .schedule_save(session.settings_path.as_deref());
            }
            options::PanelActionOutcome::UnknownField => {
                log::warn!("[UI] ui.accessibility.{op}.{field}: no accessibility field `{field}`");
            }
            options::PanelActionOutcome::UnknownOp | options::PanelActionOutcome::MismatchedOp => {
                log::warn!(
                    "[UI] ui.accessibility.{op}.{field}: `{op}` does not apply to `{field}`; toggles cycle, numeric fields increase or decrease"
                );
            }
        }
    }

    /// Route a focused slider's captured step to its field's step action when
    /// the slider sits in an engine-tier tree and binds a readonly
    /// `accessibility.*` slot. Returns `true` when routed; any other slider
    /// keeps the ordinary `setState` path, where a readonly slot warns and
    /// no-ops.
    pub(crate) fn route_engine_accessibility_slider(
        &mut self,
        slot: &str,
        owner: Option<&FocusRectOwner>,
        current: f32,
        next: f32,
    ) -> bool {
        let Some(field) = slot.strip_prefix("accessibility.") else {
            return false;
        };
        let engine_owned = owner.is_some_and(|owner| owner.tier == ScopeTier::Engine);
        if !engine_owned || !options::is_numeric_field(field) {
            return false;
        }
        let owner_is_active = self.session.as_ref().is_some_and(|session| {
            owner
                .is_some_and(|owner| session.modal_stack.active_name() == Some(owner.name.as_str()))
        });
        if !owner_is_active {
            log::warn!(
                "[UI] ignoring accessibility slider step for `{field}`: its tree is no longer the active tree"
            );
            return true;
        }
        let op = if next > current {
            "increase"
        } else if next < current {
            "decrease"
        } else {
            return true;
        };
        self.apply_accessibility_field_action(op, field);
        true
    }

    /// Whether the engine panel is anywhere on the stack.
    pub(crate) fn accessibility_panel_is_open(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .modal_stack
                .contains_pushed(ACCESSIBILITY_PANEL_NAME)
        })
    }

    /// The panel closed this frame, by any close path. Write the first-launch
    /// record and flush the pending settled save, so a panel write persists
    /// immediately when the panel closes first. A file that could not be read
    /// or parsed is never replaced, so there the record stays unwritten and the
    /// panel shows again next launch.
    pub(crate) fn note_accessibility_panel_closed(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let settings_path = session.settings_path.as_deref();
        if !session.player_options.accessibility_panel_shown {
            session.player_options.accessibility_panel_shown = true;
            session
                .player_options
                .mark_written(options::keys::ACCESSIBILITY_PANEL_SHOWN);
            session.options_bridge.schedule_save(settings_path);
        }
        session
            .options_bridge
            .flush_on_options_close(&session.player_options, settings_path);
    }
}
