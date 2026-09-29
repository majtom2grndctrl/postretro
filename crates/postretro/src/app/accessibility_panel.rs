// Engine accessibility panel: `ui.openAccessibility`, the reserved field-action
// family, engine-routed sliders, the close-time save and first-launch record,
// and the load-time check that the mod offers accessibility somewhere.
// See: context/lib/ui.md §4.1 · context/lib/player_options.md §5

use postretro_ui::demo::ACCESSIBILITY_PANEL_NAME;
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::tree::FocusRectOwner;

use crate::*;

/// Load-time check that the mounted mod offers accessibility somewhere: a mod-
/// or level-scope tree with a button that opens the engine panel or fires a
/// field action, or an engine fallback menu in use (each carries the entry).
/// Evaluated when the tree set changes — mod init, staged reload, level
/// install — never per frame. It keeps the last verdict, so it warns once per
/// state: on the first evaluation that finds no entry, and again only after an
/// entry appeared and was later removed.
#[derive(Debug, Default)]
pub(crate) struct AccessibilityEntryCheck {
    offered: Option<bool>,
}

impl AccessibilityEntryCheck {
    /// `frontend_menu_tree` is the committed frontend declaration's menu tree
    /// name (the engine fallback's name when none is declared).
    pub(crate) fn evaluate(
        &mut self,
        modal_stack: &postretro_ui::modal_stack::ModalStack,
        frontend_menu_tree: &str,
    ) {
        let offered = accessibility_is_offered(modal_stack, frontend_menu_tree);
        if !offered && self.offered != Some(false) {
            log::warn!(
                "[UI] no mod or level UI tree offers accessibility settings: no button's onPress is 'ui.openAccessibility' or a 'ui.accessibility.<op>.<field>' action. Add one to a menu players can reach, such as the pause or options menu. Players still see the accessibility panel on first launch."
            );
        }
        self.offered = Some(offered);
    }
}

fn accessibility_is_offered(
    modal_stack: &postretro_ui::modal_stack::ModalStack,
    frontend_menu_tree: &str,
) -> bool {
    use postretro_ui::actions::tree_offers_accessibility;
    // An undeclared or unregistered frontend menu presents the engine fallback.
    let frontend = if modal_stack.resolve_with_tier(frontend_menu_tree).is_some() {
        frontend_menu_tree
    } else {
        postretro_ui::demo::FRONTEND_MENU_NAME
    };
    let menu_in_use_offers = [frontend, postretro_ui::demo::PAUSE_MENU_NAME]
        .into_iter()
        .filter_map(|name| modal_stack.resolve_with_tier(name))
        .any(|(_, tree)| tree_offers_accessibility(tree));
    menu_in_use_offers
        || modal_stack
            .resolved_trees()
            .any(|(tier, tree)| tier != ScopeTier::Engine && tree_offers_accessibility(tree))
}

impl crate::session::Session {
    /// Re-evaluate the accessibility-entry check after the UI tree set or the
    /// frontend declaration changed.
    pub(crate) fn check_accessibility_entry(&mut self) {
        let frontend = self
            .frontend
            .as_ref()
            .map_or(postretro_ui::demo::FRONTEND_MENU_NAME, |f| {
                f.menu_tree.as_str()
            });
        self.accessibility_entry_check
            .evaluate(&self.modal_stack, frontend);
    }
}

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
            self.accessibility_panel_was_open = true;
        }
    }

    /// A button fired `ui.accessibility.<op>.<field>`, from any tree. Writes the
    /// store and schedules the settled save; the options bridge projects the
    /// change into `accessibility.*` and reseeds the working copy later this
    /// frame. Only button activations and engine-routed slider steps reach this
    /// handler, so it is the one store write site for the flash limiter, which
    /// has no working copy: no reaction, manifest field or script reaches it.
    pub(crate) fn apply_accessibility_field_action(&mut self, op: &str, field: &str) {
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
        // Name and tier both: a same-named tree at another tier is not the tree
        // that exported it.
        let owner_is_active = self.session.as_ref().is_some_and(|session| {
            owner.is_some_and(|owner| {
                session.modal_stack.active_name() == Some(owner.name.as_str())
                    && session.modal_stack.active_tier() == Some(owner.tier)
            })
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
