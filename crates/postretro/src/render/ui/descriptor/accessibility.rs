// Accessibility role vocabulary for widgets (M13 G2): the closed `Role` enum and
// the pure `implicit_role` that derives each widget kind's default role. A11y
// metadata is descriptor data — name/role/state — that a later task projects to
// the platform a11y layer; this module owns only the role taxonomy and its
// kind→role default map.
// See: context/lib/ui.md §4

use serde::{Deserialize, Serialize};

use super::widgets::Widget;

/// Accessibility role of a widget (M13 G2). A closed serde enum: each variant maps
/// to a camelCase wire literal. Authored as the optional `role` override on a
/// widget; absent leaves the widget at its [`implicit_role`]. The set covers the
/// kinds the a11y plan names — interactive controls, the tab/list patterns, and
/// the structural `group`/`none` defaults. A `role` override never introduces a
/// name requirement (the name precondition keys off whether the widget is
/// interactive, not its role).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Tab,
    Tablist,
    Checkbox,
    Radio,
    Listitem,
    Button,
    Slider,
    Progressbar,
    Image,
    Group,
    None,
}

/// The implicit a11y role of a widget kind (M13 G2), used when no `role` override
/// is authored. Pure — it maps the kind to its default role and nothing else:
/// `Button`→`button`, `Slider`→`slider`, `Bar`→`progressbar`, `Image`→`image`,
/// the containers (`vstack`/`hstack`/`grid`)→`group`, and `Text`/`Spacer`/
/// `Announce`→`none` (no inherent interactive/structural role).
//
// Consumed by later G2 tasks (role projection); only the unit test calls it for
// now, so it is dead outside `cfg(test)` until then.
#[cfg_attr(not(test), allow(dead_code))]
pub fn implicit_role(widget: &Widget) -> Role {
    match widget {
        Widget::Button(_) => Role::Button,
        Widget::Slider(_) => Role::Slider,
        Widget::Bar(_) => Role::Progressbar,
        Widget::Image(_) => Role::Image,
        Widget::VStack(_) | Widget::HStack(_) | Widget::Grid(_) => Role::Group,
        Widget::Text(_) | Widget::Spacer(_) | Widget::Panel(_) | Widget::Announce(_) => Role::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Widget` of each kind from minimal wire JSON for the role map test.
    fn widget(json: &str) -> Widget {
        serde_json::from_str(json).expect("fixture must deserialize")
    }

    #[test]
    fn implicit_role_maps_each_kind_to_its_default() {
        // The kind→role default map (M13 G2). Interactive controls and the passive
        // bar carry their own roles; containers are `group`; the rest are `none`.
        let cases: &[(&str, Role)] = &[
            (
                r#"{"kind":"button","id":"b","label":"X","onPress":"go"}"#,
                Role::Button,
            ),
            (
                r#"{"kind":"slider","id":"s","label":"V","bind":{"slot":"a.b"},"min":0.0,"max":1.0,"step":0.1}"#,
                Role::Slider,
            ),
            (
                r#"{"kind":"bar","bind":{"slot":"a.b"},"max":1.0,"fill":[0.0,0.0,0.0,1.0],"background":[0.0,0.0,0.0,1.0]}"#,
                Role::Progressbar,
            ),
            (
                r#"{"kind":"image","asset":"x","decorative":true}"#,
                Role::Image,
            ),
            (
                r#"{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","children":[]}"#,
                Role::Group,
            ),
            (
                r#"{"kind":"hstack","gap":0.0,"padding":0.0,"align":"start","children":[]}"#,
                Role::Group,
            ),
            (
                r#"{"kind":"grid","gap":0.0,"padding":0.0,"align":"start","cols":1,"children":[]}"#,
                Role::Group,
            ),
            (
                r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#,
                Role::None,
            ),
            (r#"{"kind":"spacer","flexGrow":1.0}"#, Role::None),
            (
                r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null}"#,
                Role::None,
            ),
            (r#"{"kind":"announce","text":"hi"}"#, Role::None),
        ];
        for (json, expected) in cases {
            assert_eq!(
                implicit_role(&widget(json)),
                *expected,
                "implicit_role mismatch for: {json}"
            );
        }
    }
}
