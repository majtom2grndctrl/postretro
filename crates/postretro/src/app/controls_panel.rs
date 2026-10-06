// Engine controls panel: `ui.openControls`, the rows built from the effective
// table, the capture prompt and its raw-input intake, conflict and refusal
// dialogs, reset, and the saved player rows.
// See: context/lib/input.md §2 · context/lib/player_options.md §6 · context/lib/ui.md §4.1

use postretro_ui::actions::ControlsAction;
use postretro_ui::demo::{CONTROLS_CAPTURE_NAME, CONTROLS_DIALOG_NAME, CONTROLS_PANEL_NAME};
use postretro_ui::descriptor::AnchoredTree;
use serde_json::{Value, json};

use crate::input::{
    ActivatorKind, AuthorLayer, BindingCapture, CaptureTarget, Command, CommandContext,
    DeviceClass, EffectiveTable, PhysicalInput, PlayerLayer, RebindProposal, Relevance,
    input_label, input_name, propose_rebind, reset_command,
};
use crate::*;

/// Slots each device class shows at least, so an unbound command still offers
/// a primary and a secondary binding.
const MIN_SLOTS: usize = 2;

/// The panel's live state: the open capture prompt, a conflicting binding
/// awaiting the player's answer, and the table generation the pushed panel
/// was built from.
#[derive(Debug, Default)]
pub(crate) struct ControlsPanelState {
    capture: Option<BindingCapture>,
    pending_replace: Option<PlayerLayer>,
    built_generation: Option<u64>,
    /// The commands the pushed panel lists, in row order.
    listed: Vec<Command>,
    /// The row focus belonged to when a prompt closed because its command left
    /// the list; the next rebuild focuses the nearest remaining row.
    focus_anchor: Option<Command>,
}

/// One listed command: its presentation and its effective inputs per class.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ControlsRow {
    pub(crate) command: Command,
    pub(crate) label: String,
    pub(crate) category: String,
    /// Keyboard-and-mouse inputs, then gamepad inputs, each in slot order,
    /// with the activator each binding uses.
    pub(crate) inputs: [Vec<(PhysicalInput, ActivatorKind)>; 2],
    /// A player binding took one of this command's newer author defaults.
    pub(crate) displaced: bool,
}

impl ControlsPanelState {
    /// Offer a raw press to an open capture prompt. The caller checked the
    /// prompt is the active tree (`Session::capture_prompt_is_active`).
    pub(crate) fn offer_press(&mut self, input: PhysicalInput) {
        if let Some(capture) = self.capture.as_mut() {
            capture.offer_press(input);
        }
    }
}

fn class_index(class: DeviceClass) -> usize {
    match class {
        DeviceClass::KeyboardMouse => 0,
        DeviceClass::Gamepad => 1,
    }
}

fn class_from_key(key: &str) -> Option<DeviceClass> {
    DeviceClass::ALL
        .into_iter()
        .find(|class| class.settings_key() == key)
}

fn class_label(class: DeviceClass) -> &'static str {
    match class {
        DeviceClass::KeyboardMouse => "KEYBOARD AND MOUSE",
        DeviceClass::Gamepad => "GAMEPAD",
    }
}

/// The label a command shows: the author's, else its ID in words.
pub(crate) fn command_label(author: &AuthorLayer, command: Command) -> String {
    author
        .presentation
        .get(&command)
        .and_then(|p| p.label.clone())
        .unwrap_or_else(|| command.id().replace('_', " ").to_uppercase())
}

fn command_category(author: &AuthorLayer, command: Command) -> String {
    author
        .presentation
        .get(&command)
        .and_then(|p| p.category.clone())
        .unwrap_or_else(|| match command.context() {
            CommandContext::Gameplay => "GAMEPLAY".to_string(),
            CommandContext::Ui => "MENUS".to_string(),
        })
}

/// The rows the panel lists: relevant commands only, ordered by the author's
/// `order`, then manifest order, then the engine's command order, and grouped
/// by category in the order each category first appears.
pub(crate) fn controls_rows(table: &EffectiveTable, author: &AuthorLayer) -> Vec<ControlsRow> {
    let mut commands: Vec<Command> = Command::ALL
        .into_iter()
        .filter(|command| table.relevance(*command) == Relevance::Relevant)
        .collect();
    let order = |command: &Command| {
        author
            .presentation
            .get(command)
            .and_then(|p| p.order)
            .unwrap_or(f64::MAX)
    };
    let manifest = |command: &Command| {
        author
            .manifest_order
            .iter()
            .position(|c| c == command)
            .unwrap_or(usize::MAX)
    };
    commands.sort_by(|a, b| {
        order(a)
            .total_cmp(&order(b))
            .then(manifest(a).cmp(&manifest(b)))
    });
    let rows: Vec<ControlsRow> = commands
        .into_iter()
        .map(|command| {
            let inputs = DeviceClass::ALL.map(|class| {
                table
                    .entries()
                    .iter()
                    .filter(|e| e.command == command && e.class == class)
                    .map(|e| (e.input, e.activator.kind))
                    .collect()
            });
            ControlsRow {
                command,
                label: command_label(author, command),
                category: command_category(author, command),
                inputs,
                displaced: table.displaced().iter().any(|(c, _)| *c == command),
            }
        })
        .collect();
    // Stable grouping: each category keeps its rows' order and sits where its
    // first row did.
    let mut categories: Vec<String> = Vec::new();
    for row in &rows {
        if !categories.contains(&row.category) {
            categories.push(row.category.clone());
        }
    }
    categories
        .iter()
        .flat_map(|category| rows.iter().filter(move |row| &row.category == category))
        .cloned()
        .collect()
}

fn activator_suffix(kind: ActivatorKind) -> &'static str {
    match kind {
        ActivatorKind::Press => "",
        ActivatorKind::Release => " (RELEASE)",
        ActivatorKind::Tap => " (TAP)",
        ActivatorKind::Hold => " (HOLD)",
    }
}

/// A slot button's id.
pub(crate) fn slot_id(command: Command, class: DeviceClass, slot: usize) -> String {
    format!("ctl_{}_{}_{slot}", command.id(), class.settings_key())
}

fn text(content: &str, size: f32) -> Value {
    json!({ "kind": "text", "content": content, "fontSize": size, "color": "ok", "font": "mono" })
}

fn button(id: &str, label: &str, on_press: &str) -> Value {
    json!({ "kind": "button", "id": id, "label": label, "onPress": on_press })
}

fn spacer() -> Value {
    json!({ "kind": "spacer", "flexGrow": 0.0 })
}

fn tree(initial_focus: Option<&str>, children: Vec<Value>) -> AnchoredTree {
    let mut value = json!({
        "anchor": "center",
        "offset": [0.0, 0.0],
        "captureMode": "capture",
        "root": {
            "kind": "vstack",
            "gap": 12.0,
            "padding": 20.0,
            "align": "start",
            "fill": "panel.default",
            "focus": { "policy": "linear" },
            "children": children,
        },
    });
    if let Some(focus) = initial_focus {
        value["initialFocus"] = json!(focus);
    }
    serde_json::from_value(value).expect("engine controls descriptor is well formed")
}

/// The controls panel. `initial_focus` overrides the first row's first slot.
pub(crate) fn build_controls_panel(rows: &[ControlsRow], initial_focus: Option<&str>) -> AnchoredTree {
    let slots = DeviceClass::ALL.map(|class| {
        rows.iter()
            .map(|row| row.inputs[class_index(class)].len())
            .max()
            .unwrap_or(0)
            .max(MIN_SLOTS)
    });
    let cols = 2 + slots[0] + slots[1];

    let mut cells = vec![text("COMMAND", 14.0)];
    for (class, count) in DeviceClass::ALL.into_iter().zip(slots) {
        let short = match class {
            DeviceClass::KeyboardMouse => "KEY",
            DeviceClass::Gamepad => "PAD",
        };
        cells.extend((1..=count).map(|n| text(&format!("{short} {n}"), 14.0)));
    }
    cells.push(spacer());

    let mut category = None;
    for row in rows {
        if category != Some(&row.category) {
            category = Some(&row.category);
            cells.push(text(&row.category, 18.0));
            cells.extend((1..cols).map(|_| spacer()));
        }
        let marker = if row.displaced { " !" } else { "" };
        cells.push(text(&format!("{}{marker}", row.label), 16.0));
        for (class, count) in DeviceClass::ALL.into_iter().zip(slots) {
            for slot in 0..count {
                let label = match row.inputs[class_index(class)].get(slot) {
                    Some((input, kind)) => {
                        format!("{}{}", input_label(*input), activator_suffix(*kind))
                    }
                    None => "---".to_string(),
                };
                cells.push(button(
                    &slot_id(row.command, class, slot),
                    &label,
                    &format!(
                        "{}capture.{}.{}.{slot}",
                        postretro_ui::actions::CONTROLS_ACTION_PREFIX,
                        row.command.id(),
                        class.settings_key()
                    ),
                ));
            }
        }
        cells.push(button(
            &format!("ctl_{}_reset", row.command.id()),
            "RESET",
            &format!(
                "{}reset.{}",
                postretro_ui::actions::CONTROLS_ACTION_PREFIX,
                row.command.id()
            ),
        ));
    }

    let mut children = vec![
        text("CONTROLS", 24.0),
        json!({
            "kind": "grid",
            "gap": 8.0,
            "padding": 0.0,
            "align": "center",
            "cols": cols,
            "focus": { "policy": "spatial" },
            "children": cells,
        }),
    ];
    if rows.iter().any(|row| row.displaced) {
        children.push(text(
            "! A DEFAULT FOR THIS COMMAND IS TAKEN BY ONE OF YOUR BINDINGS",
            14.0,
        ));
    }
    children.push(json!({
        "kind": "hstack",
        "gap": 12.0,
        "padding": 0.0,
        "align": "start",
        "children": [
            button(
                "ctl_resetAll",
                "RESET ALL",
                &format!("{}resetAll", postretro_ui::actions::CONTROLS_ACTION_PREFIX),
            ),
            button("ctl_back", "BACK", postretro_ui::actions::CLOSE_DIALOG_ACTION),
        ],
    }));
    let first = rows
        .first()
        .map(|row| slot_id(row.command, DeviceClass::KeyboardMouse, 0))
        .unwrap_or_else(|| "ctl_back".to_string());
    tree(Some(initial_focus.unwrap_or(&first)), children)
}

/// The capture prompt. It has no focus stops: every input it sees is captured.
pub(crate) fn build_capture_prompt(
    label: &str,
    target: CaptureTarget,
    current: Option<PhysicalInput>,
) -> AnchoredTree {
    let mut children = vec![
        text(&format!("PRESS AN INPUT FOR {label}"), 22.0),
        text(
            &format!("{} SLOT {}", class_label(target.class), target.slot + 1),
            16.0,
        ),
    ];
    children.push(text(
        &match current {
            Some(input) => format!("PRESS {} AGAIN TO KEEP IT", input_label(input)),
            None => "RESET RETURNS THIS COMMAND TO ITS DEFAULTS".to_string(),
        },
        14.0,
    ));
    tree(None, children)
}

fn dialog(lines: &[String], buttons: Vec<Value>, initial_focus: &str) -> AnchoredTree {
    let mut children: Vec<Value> = lines.iter().map(|line| text(line, 16.0)).collect();
    children.push(json!({
        "kind": "hstack",
        "gap": 12.0,
        "padding": 0.0,
        "align": "start",
        "children": buttons,
    }));
    tree(Some(initial_focus), children)
}

/// Asks whether a conflicting binding takes its input from the commands that
/// hold it. Keeping the current bindings is the safe choice.
pub(crate) fn build_conflict_dialog(input: PhysicalInput, holders: &[String]) -> AnchoredTree {
    dialog(
        &[
            format!("{} ALSO DRIVES {}", input_label(input), holders.join(", ")),
            "REPLACE TAKES IT FROM THEM".to_string(),
        ],
        vec![
            button(
                "ctl_keep",
                "KEEP CURRENT",
                &format!("{}keep", postretro_ui::actions::CONTROLS_ACTION_PREFIX),
            ),
            button(
                "ctl_replace",
                "REPLACE",
                &format!("{}replace", postretro_ui::actions::CONTROLS_ACTION_PREFIX),
            ),
        ],
        "ctl_keep",
    )
}

/// Explains a binding the guard refused.
pub(crate) fn build_refusal_dialog(unbound: &str, class: DeviceClass) -> AnchoredTree {
    dialog(
        &[
            format!("{unbound} WOULD HAVE NO {} BINDING", class_label(class)),
            "CONFIRM, CANCEL AND MENU ALWAYS KEEP ONE".to_string(),
        ],
        vec![button(
            "ctl_ok",
            "OK",
            postretro_ui::actions::CLOSE_DIALOG_ACTION,
        )],
        "ctl_ok",
    )
}

/// The saved rows a player-layer change writes: each changed row's input
/// names, or `None` to remove a row so the command follows the author again.
/// Ordered by command and class so saves are deterministic.
pub(crate) fn changed_rows(
    old: &PlayerLayer,
    new: &PlayerLayer,
) -> Vec<(DeviceClass, Command, Option<Vec<String>>)> {
    let mut keys: Vec<(Command, DeviceClass)> =
        old.rows.keys().chain(new.rows.keys()).copied().collect();
    keys.sort_by_key(|(command, class)| (command.id(), class.settings_key()));
    keys.dedup();
    keys.into_iter()
        .filter(|key| old.rows.get(key) != new.rows.get(key))
        .map(|key @ (command, class)| {
            let names = new.rows.get(&key).map(|row| {
                row.iter()
                    .flatten()
                    .filter_map(|input| input_name(*input).map(str::to_string))
                    .collect()
            });
            (class, command, names)
        })
        .collect()
}

impl crate::session::Session {
    /// Whether the capture prompt is the active tree: every raw input goes to
    /// it, and none reaches nav, the menu toggle, or gameplay.
    pub(crate) fn capture_prompt_is_active(&self) -> bool {
        self.controls.capture.is_some()
            && self.modal_stack.active_name() == Some(CONTROLS_CAPTURE_NAME)
    }

    /// Offer a raw press to the active capture prompt. Returns whether the
    /// prompt took it, in which case the caller routes it nowhere else.
    pub(crate) fn offer_capture_press(&mut self, input: PhysicalInput) -> bool {
        if !self.capture_prompt_is_active() {
            return false;
        }
        if let Some(capture) = self.controls.capture.as_mut() {
            capture.offer_press(input);
        }
        true
    }

    /// Offer raw mouse motion to the active capture prompt.
    pub(crate) fn offer_capture_mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.capture_prompt_is_active()
            && let Some(capture) = self.controls.capture.as_mut()
        {
            capture.offer_mouse_motion(dx, dy);
        }
    }

    /// Close the capture prompt without binding anything.
    pub(crate) fn abandon_capture(&mut self) {
        if self.controls.capture.take().is_some() {
            pop_named(&mut self.modal_stack, CONTROLS_CAPTURE_NAME);
        }
    }
}

/// Pop `name` when it is the top tree.
fn pop_named(stack: &mut postretro_ui::modal_stack::ModalStack, name: &str) {
    if stack.active_name() == Some(name) {
        stack.pop();
    }
}

impl App {
    /// `ui.openControls`: push the engine panel over whatever shows. A panel
    /// already on the stack is not pushed twice.
    pub(crate) fn open_controls_panel(&mut self) {
        self.refresh_effective_bindings();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.modal_stack.contains_pushed(CONTROLS_PANEL_NAME) {
            return;
        }
        let rows = controls_rows(session.bindings.table(), session.bindings.author());
        let panel = build_controls_panel(&rows, None);
        session.controls.listed = rows.iter().map(|row| row.command).collect();
        session.controls.built_generation = Some(session.bindings.generation());
        session.modal_stack.push(CONTROLS_PANEL_NAME, panel);
    }

    /// A `ui.controls.*` action from the panel or its dialogs.
    pub(crate) fn apply_controls_action(&mut self, action: ControlsAction<'_>) {
        match action {
            ControlsAction::Capture {
                command,
                class,
                slot,
            } => {
                let (Some(command), Some(class)) =
                    (Command::from_id(command), class_from_key(class))
                else {
                    log::warn!("[UI] ui.controls.capture: no command `{command}` on `{class}`");
                    return;
                };
                self.open_capture_prompt(CaptureTarget {
                    command,
                    class,
                    slot,
                });
            }
            ControlsAction::Reset { command } => {
                let Some(command) = Command::from_id(command) else {
                    log::warn!("[UI] ui.controls.reset: no command `{command}`");
                    return;
                };
                let Some(session) = self.session.as_ref() else {
                    return;
                };
                let player = reset_command(
                    session.bindings.player(),
                    command,
                    session.bindings.swap_confirm_cancel(),
                );
                self.apply_player_layer(player);
            }
            ControlsAction::ResetAll => self.apply_player_layer(PlayerLayer::default()),
            ControlsAction::Replace => {
                let Some(session) = self.session.as_mut() else {
                    return;
                };
                let pending = session.controls.pending_replace.take();
                pop_named(&mut session.modal_stack, CONTROLS_DIALOG_NAME);
                if let Some(player) = pending {
                    self.apply_player_layer(player);
                }
            }
            ControlsAction::Keep => {
                if let Some(session) = self.session.as_mut() {
                    session.controls.pending_replace = None;
                    pop_named(&mut session.modal_stack, CONTROLS_DIALOG_NAME);
                }
            }
        }
    }

    fn open_capture_prompt(&mut self, target: CaptureTarget) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let table = session.bindings.table();
        if table.relevance(target.command) != Relevance::Relevant {
            return;
        }
        let current = table.inputs(target.command, target.class).get(target.slot).copied();
        let label = command_label(session.bindings.author(), target.command);
        session.controls.capture = Some(BindingCapture::new(target));
        session.modal_stack.push(
            CONTROLS_CAPTURE_NAME,
            build_capture_prompt(&label, target, current),
        );
    }

    /// Per frame, after the frame's activations: close a prompt whose command
    /// became irrelevant (P25), resolve a captured input, and rebuild the
    /// pushed panel when the effective table changed.
    pub(crate) fn update_controls_panel(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // A prompt or dialog closed by any other path drops its state.
        if !session.modal_stack.contains_pushed(CONTROLS_CAPTURE_NAME) {
            session.controls.capture = None;
        }
        if !session.modal_stack.contains_pushed(CONTROLS_DIALOG_NAME) {
            session.controls.pending_replace = None;
        }
        if let Some(target) = session.controls.capture.as_ref().map(BindingCapture::target)
            && session.bindings.table().relevance(target.command) != Relevance::Relevant
        {
            session.controls.focus_anchor = Some(target.command);
            session.abandon_capture();
        }
        self.resolve_binding_capture();
        self.rebuild_controls_panel();
    }

    fn resolve_binding_capture(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.capture_prompt_is_active() {
            return;
        }
        let Some(capture) = session.controls.capture.as_mut() else {
            return;
        };
        let target = capture.target();
        let Some(input) = capture.take_candidate() else {
            return;
        };
        session.abandon_capture();
        let bindings = &session.bindings;
        let proposal = propose_rebind(
            bindings.table(),
            bindings.author(),
            bindings.player(),
            bindings.facts(),
            bindings.swap_confirm_cancel(),
            target.command,
            target.class,
            target.slot,
            input,
        );
        match proposal {
            RebindProposal::Unchanged => {}
            RebindProposal::Clean { player } => self.apply_player_layer(player),
            RebindProposal::Conflict { with, replace } => {
                let holders: Vec<String> = with
                    .iter()
                    .map(|command| command_label(session.bindings.author(), *command))
                    .collect();
                session.controls.pending_replace = Some(replace);
                session
                    .modal_stack
                    .push(CONTROLS_DIALOG_NAME, build_conflict_dialog(input, &holders));
            }
            RebindProposal::Refused { unbound } => {
                let label = command_label(session.bindings.author(), unbound);
                session.modal_stack.push(
                    CONTROLS_DIALOG_NAME,
                    build_refusal_dialog(&label, target.class),
                );
            }
        }
    }

    /// Replace the pushed panel's descriptor when the table changed since it
    /// was built. Focus on a row that left the list moves to the nearest row
    /// that remains.
    fn rebuild_controls_panel(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.modal_stack.contains_pushed(CONTROLS_PANEL_NAME)
            || session.controls.built_generation == Some(session.bindings.generation())
        {
            return;
        }
        let rows = controls_rows(session.bindings.table(), session.bindings.author());
        let listed: Vec<Command> = rows.iter().map(|row| row.command).collect();
        let anchor = session.controls.focus_anchor.take().or_else(|| {
            let id = self.ui_focused_id.as_deref()?;
            session
                .controls
                .listed
                .iter()
                .copied()
                .find(|command| id.starts_with(&format!("ctl_{}_", command.id())))
        });
        let old = &session.controls.listed;
        let initial_focus = anchor
            .filter(|command| !listed.contains(command))
            .and_then(|command| old.iter().position(|c| *c == command))
            .and_then(|index| {
                old[index..]
                    .iter()
                    .chain(old[..index].iter().rev())
                    .find(|command| listed.contains(command))
            })
            .map(|command| slot_id(*command, DeviceClass::KeyboardMouse, 0));
        let panel = build_controls_panel(&rows, initial_focus.as_deref());
        session
            .modal_stack
            .replace_pushed_descriptor(CONTROLS_PANEL_NAME, &panel);
        session.controls.listed = listed;
        session.controls.built_generation = Some(session.bindings.generation());
    }

    /// Install a new player layer: save the rows that changed under the
    /// committed mod id, rebuild the table, and refresh the panel.
    pub(crate) fn apply_player_layer(&mut self, player: PlayerLayer) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() {
            let mod_id = mod_id.to_string();
            for (class, command, names) in changed_rows(session.bindings.player(), &player) {
                session.player_options.set_game_binding_row(
                    &mod_id,
                    class.settings_key(),
                    command.id(),
                    names,
                );
            }
            session
                .options_bridge
                .schedule_save(session.settings_path.as_deref());
        }
        session.bindings.set_player_layer(player);
        self.refresh_effective_bindings();
        self.rebuild_controls_panel();
    }
}

#[cfg(test)]
#[path = "controls_panel_tests.rs"]
mod tests;
