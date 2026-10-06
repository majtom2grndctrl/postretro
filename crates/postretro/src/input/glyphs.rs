// What a `Glyph({ command })` draws: the command's first effective binding on
// the current device family, as the mod's art or the input's name.
// See: context/lib/input.md §2 · context/lib/ui.md §4

use super::binding_table::{EffectiveTable, GlyphDirs};
use super::commands::Command;
use super::device_family::DeviceFamily;
use super::input_names::{input_label, input_name};
use super::relevance::Relevance;

/// One command's glyph for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphView {
    /// The mod's art, by UI image key `<dir>/<input>`.
    Art(String),
    /// No art for the input: draw its name.
    Label(String),
}

/// The art directory a family reads.
pub fn glyph_dir(glyphs: &GlyphDirs, family: DeviceFamily) -> Option<&str> {
    match family {
        DeviceFamily::KeyboardMouse => glyphs.keyboard_mouse.as_deref(),
        DeviceFamily::Xbox => glyphs.xbox.as_deref(),
        DeviceFamily::PlayStation => glyphs.playstation.as_deref(),
        DeviceFamily::Nintendo => glyphs.nintendo.as_deref(),
    }
}

/// The image key of an input's art in `dir`.
pub fn glyph_key(dir: &str, input: &str) -> String {
    format!("{}/{input}", dir.trim_end_matches('/'))
}

/// Resolve `command`'s glyph on `family`. An irrelevant command, or one
/// unbound on the family's class, draws nothing. The table already applies
/// player rows and the confirm/cancel swap, so the glyph follows both.
pub fn resolve_glyph(
    table: &EffectiveTable,
    glyphs: &GlyphDirs,
    family: DeviceFamily,
    command: Command,
    has_art: impl Fn(&str) -> bool,
) -> Option<GlyphView> {
    if table.relevance(command) != Relevance::Relevant {
        return None;
    }
    let input = *table.inputs(command, family.class()).first()?;
    let art = glyph_dir(glyphs, family)
        .zip(input_name(input))
        .map(|(dir, name)| glyph_key(dir, name))
        .filter(|key| has_art(key));
    Some(match art {
        Some(key) => GlyphView::Art(key),
        None => GlyphView::Label(input_label(input)),
    })
}

#[cfg(test)]
mod tests {
    use gilrs::Button;

    use super::*;
    use crate::input::binding_table::{AuthorLayer, PlayerLayer};
    use crate::input::input_names::DeviceClass;
    use crate::input::relevance::RelevanceFacts;
    use crate::input::types::PhysicalInput;

    fn dirs() -> GlyphDirs {
        GlyphDirs {
            keyboard_mouse: Some("ui/glyphs/kbm".into()),
            xbox: Some("ui/glyphs/xbox".into()),
            playstation: Some("ui/glyphs/ps/".into()),
            nintendo: None,
        }
    }

    fn table(player: &PlayerLayer, swap: bool) -> EffectiveTable {
        EffectiveTable::build(&AuthorLayer::default(), player, RelevanceFacts::default(), swap)
    }

    #[test]
    fn a_glyph_draws_the_first_binding_on_the_family_from_its_art() {
        let table = table(&PlayerLayer::default(), false);
        let all_art = |_: &str| true;
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::Xbox, Command::NavConfirm, all_art),
            Some(GlyphView::Art("ui/glyphs/xbox/south".into()))
        );
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::PlayStation, Command::NavConfirm, all_art),
            Some(GlyphView::Art("ui/glyphs/ps/south".into()))
        );
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::KeyboardMouse, Command::NavConfirm, all_art),
            Some(GlyphView::Art("ui/glyphs/kbm/Enter".into()))
        );
    }

    #[test]
    fn missing_art_draws_the_input_name() {
        let table = table(&PlayerLayer::default(), false);
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::Nintendo, Command::NavCancel, |_| true),
            Some(GlyphView::Label("EAST".into())),
            "the block names no Nintendo art"
        );
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::Xbox, Command::NavCancel, |_| false),
            Some(GlyphView::Label("EAST".into()))
        );
    }

    #[test]
    fn irrelevant_or_unbound_commands_draw_nothing() {
        let table = table(&PlayerLayer::default(), false);
        assert_eq!(
            resolve_glyph(&table, &dirs(), DeviceFamily::Xbox, Command::Dash, |_| true),
            None,
            "dash is irrelevant without a dash descriptor"
        );
        assert_eq!(
            resolve_glyph(
                &table,
                &dirs(),
                DeviceFamily::Xbox,
                Command::SelectWieldable1,
                |_| true
            ),
            None,
            "unbound on the gamepad"
        );
    }

    #[test]
    fn glyphs_follow_rebinding_and_the_swap() {
        let mut player = PlayerLayer::default();
        player.rows.insert(
            (Command::NavConfirm, DeviceClass::Gamepad),
            vec![Some(PhysicalInput::GamepadButton(Button::West))],
        );
        let rebound = table(&player, false);
        assert_eq!(
            resolve_glyph(&rebound, &dirs(), DeviceFamily::Xbox, Command::NavConfirm, |_| true),
            Some(GlyphView::Art("ui/glyphs/xbox/west".into()))
        );
        let swapped = table(&PlayerLayer::default(), true);
        assert_eq!(
            resolve_glyph(&swapped, &dirs(), DeviceFamily::Xbox, Command::NavConfirm, |_| true),
            Some(GlyphView::Art("ui/glyphs/xbox/east".into()))
        );
    }
}
