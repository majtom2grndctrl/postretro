// UI descriptor data is owned by scripting-core so script manifests and renderer
// trees share one concrete type.
// See: context/lib/scripting.md §12

pub use postretro_scripting_core::ui::descriptor::*;

#[cfg(test)]
mod tests {
    use super::*;
    // `Anchor` lives in `layout`, not the descriptor surface; the envelope embeds
    // it. The wire tests reference it directly, so pull it into test scope (it is
    // not re-exported because the descriptor API does not own it).
    use super::super::layout::Anchor;

    /// A tree exercising the seven non-interactive kinds wrapped in the placement envelope.
    /// Button/Slider/Bar (Goal F interactive widgets) are covered by their own round-trip tests.
    /// Field order matches the Rust struct declaration order so the
    /// re-serialized JSON is byte-identical to this source (serde emits fields
    /// in declaration order). The tag `kind` always serializes first.
    const ALL_KINDS_JSON: &str = r#"{"anchor":"center","offset":[10.0,-20.0],"root":{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hello","fontSize":18.0,"color":[1.0,1.0,1.0,1.0]},{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":{"texture":"ui/frame","slice":[8.0,8.0,8.0,8.0],"tint":[1.0,1.0,1.0,1.0]}},{"kind":"hstack","gap":2.0,"padding":0.0,"align":"center","children":[{"kind":"image","asset":"ui/logo"},{"kind":"spacer","flexGrow":1.0}]},{"kind":"grid","gap":1.0,"padding":3.0,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon"}]}]}}"#;

    #[test]
    fn anchored_tree_round_trips_all_seven_noninteractive_kinds_identically() {
        let tree: AnchoredTree =
            serde_json::from_str(ALL_KINDS_JSON).expect("fixture must deserialize");
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, ALL_KINDS_JSON);
    }

    #[test]
    fn empty_container_round_trips_with_explicit_children_array() {
        // An empty container must keep `"children":[]` across a round-trip —
        // no `skip_serializing_if` — so identity holds for childless stacks.
        let json = r#"{"anchor":"topLeft","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","children":[]}}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn misspelled_field_key_is_rejected_not_silently_defaulted() {
        // `deny_unknown_fields` on the per-widget field structs converts a typo'd
        // key into a hard parse error instead of silently dropping the override and
        // falling back to the field default (the pause-menu nav-override gap). Here
        // a button misspells `focusNeighbors` as `focusNeighbours`: previously this
        // deserialized fine (the override silently lost), now it is a serde error.
        let typo = r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame","focusNeighbours":{"down":"quit"}}"#;
        let result: Result<Widget, _> = serde_json::from_str(typo);
        assert!(
            result.is_err(),
            "a misspelled widget field key must be a serde error, not a silent default"
        );

        // The correctly-spelled key still parses — the guard rejects only unknowns.
        let correct = r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame","focusNeighbors":{"down":"quit"}}"#;
        serde_json::from_str::<Widget>(correct).expect("correctly-spelled key must still parse");
    }

    #[test]
    fn unknown_kind_deserializes_to_error_not_panic() {
        // An unrecognized `kind` tag is a serde error, never a panic — mod
        // authors get a rejected document, not a crash.
        let json = r#"{"kind":"carousel"}"#;
        let result: Result<Widget, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown kind must be a serde error");
    }

    #[test]
    fn anchor_serializes_to_camel_case_wire_form() {
        // Pins the cross-boundary casing: TopLeft -> "topLeft", Center ->
        // "center". The envelope reuses `layout::Anchor`.
        assert_eq!(
            serde_json::to_string(&Anchor::TopLeft).unwrap(),
            r#""topLeft""#
        );
        assert_eq!(
            serde_json::to_string(&Anchor::BottomRight).unwrap(),
            r#""bottomRight""#
        );
        assert_eq!(
            serde_json::to_string(&Anchor::Center).unwrap(),
            r#""center""#
        );
    }

    #[test]
    fn align_serializes_to_camel_case_wire_form() {
        assert_eq!(serde_json::to_string(&Align::Start).unwrap(), r#""start""#);
        assert_eq!(
            serde_json::to_string(&Align::Stretch).unwrap(),
            r#""stretch""#
        );
    }

    #[test]
    fn bound_text_round_trips_with_slot_and_format() {
        // A `text` node carrying a `bind` with both `slot` and `format` keeps its
        // camelCase wire form byte-for-byte. Field order: content, fontSize,
        // color, then the nested bind { slot, format }.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","format":"HP {}"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_text_round_trips_with_format_absent() {
        // A `bind` with no `format` omits the field entirely (skip_serializing_if).
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.ammo"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn unbound_text_serializes_without_a_bind_field() {
        // An unbound text widget must not emit a `bind` key — static widgets keep
        // their pre-binding wire form so old descriptors round-trip unchanged.
        let json = r#"{"kind":"text","content":"hello","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_panel_round_trips_with_slot() {
        // A `panel` node binding its `fill` to a color slot keeps its wire form.
        // Field order: fill, border (null when absent), then bind { slot }.
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn unbound_panel_serializes_without_a_bind_field() {
        // An unbound panel must not emit a `bind` key.
        let json = r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":null}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn text_color_token_and_literal_each_round_trip_in_its_own_form() {
        // The wire form of a color slot is disjoint: a token serializes as a bare
        // string, a literal as a bare array. Each must re-serialize byte-identically
        // to the form it was authored in — the untagged union never rewrites one
        // form into the other.
        let token = r#"{"kind":"text","content":"HP","fontSize":18.0,"color":"critical"}"#;
        let token_widget: Widget = serde_json::from_str(token).expect("token must deserialize");
        assert_eq!(serde_json::to_string(&token_widget).unwrap(), token);

        let literal = r#"{"kind":"text","content":"HP","fontSize":18.0,"color":[1.0,0.0,0.0,1.0]}"#;
        let literal_widget: Widget =
            serde_json::from_str(literal).expect("literal must deserialize");
        assert_eq!(serde_json::to_string(&literal_widget).unwrap(), literal);
    }

    #[test]
    fn color_value_parses_array_to_literal_and_string_to_token() {
        // Pin the variant the disjoint JSON forms land on (declaration order makes
        // `Literal` first, but arrays and strings are unambiguous either way).
        let lit: ColorValue = serde_json::from_str("[1.0,0.0,0.0,1.0]").unwrap();
        assert_eq!(lit, ColorValue::Literal([1.0, 0.0, 0.0, 1.0]));
        let tok: ColorValue = serde_json::from_str(r#""critical""#).unwrap();
        assert_eq!(tok, ColorValue::Token("critical".to_string()));
    }

    #[test]
    fn spacing_value_token_and_literal_each_round_trip_in_its_own_form() {
        // A spacing token serializes as a bare string, a literal as a bare JSON
        // number — `SpacingValue::Literal` wraps a bare `f32`, so `4.0` stays `4.0`.
        let token: SpacingValue = serde_json::from_str(r#""tight""#).expect("token deserializes");
        assert_eq!(token, SpacingValue::Token("tight".to_string()));
        assert_eq!(serde_json::to_string(&token).unwrap(), r#""tight""#);

        let literal: SpacingValue = serde_json::from_str("4.0").expect("literal deserializes");
        assert_eq!(literal, SpacingValue::Literal(4.0));
        assert_eq!(serde_json::to_string(&literal).unwrap(), "4.0");
    }

    #[test]
    fn container_spacing_token_round_trips_identically() {
        // A container may carry token gap/padding (bare strings) the same way it
        // carries literal numbers; the wire form stays a flat object either way.
        let json = r#"{"kind":"vstack","gap":"m","padding":"s","align":"start","children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn text_font_token_round_trips_and_absent_font_omits_the_field() {
        // A `font` token round-trips byte-identically; an absent font omits the key
        // entirely (skip_serializing_if), so pre-theming fontless text is unchanged.
        let with_font = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"font":"mono"}"#;
        let widget: Widget = serde_json::from_str(with_font).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), with_font);

        let no_font = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#;
        let widget: Widget = serde_json::from_str(no_font).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), no_font);
    }

    #[test]
    fn container_with_fill_and_border_round_trips_identically() {
        // A container carrying a backdrop `fill` + 9-slice `border` (the splash's
        // framed-panel vocabulary). Field order matches the struct declaration
        // (gap, padding, align, fill, border, children) so the re-serialized JSON
        // is byte-identical. `fill`/`border` skip-serialize when absent, so a
        // fill-less container keeps its old wire form — pinned by the all-kinds
        // and empty-container round-trips above.
        let json = r#"{"kind":"vstack","gap":0.0,"padding":4.0,"align":"center","fill":[0.1,0.55,0.62,1.0],"border":{"texture":"","slice":[12.0,12.0,12.0,12.0],"tint":[0.1,0.55,0.62,1.0]},"children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_text_round_trips_with_tween() {
        // A `text` bind carrying a `tween` keeps its camelCase wire form
        // byte-for-byte. Field order inside tween: durationMs, easing, from.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","tween":{"durationMs":1200.0,"easing":"easeOut","from":0.0}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_panel_round_trips_with_tween_and_from_absent() {
        // A `panel` bind tween with no `from` omits the `from` key entirely
        // (skip_serializing_if) and round-trips byte-identically.
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor","tween":{"durationMs":150.0,"easing":"easeInOut"}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        // Belt-and-suspenders: the absent `from` emits no `from` key.
        assert!(
            !reserialized.contains("from"),
            "absent from must emit no key"
        );
    }

    #[test]
    fn bound_panel_round_trips_with_tween_from_array() {
        // A `panel` bind tween whose `from` is a length-4 linear-RGBA array keeps
        // its wire form (the panel-side `from` type, distinct from text's number).
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor","tween":{"durationMs":300.0,"easing":"linear","from":[1.0,0.0,0.0,1.0]}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn tween_less_binds_serialize_without_a_tween_field() {
        // A bind with no `tween` must not emit a `tween` key — pre-tweening binds
        // keep their exact wire form so old descriptors round-trip unchanged.
        let text = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.ammo"}}"#;
        let widget: Widget = serde_json::from_str(text).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, text);
        assert!(
            !reserialized.contains("tween"),
            "tween-less text emits no tween key"
        );

        let panel = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor"}}"#;
        let widget: Widget = serde_json::from_str(panel).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, panel);
        assert!(
            !reserialized.contains("tween"),
            "tween-less panel emits no tween key"
        );
    }

    #[test]
    fn easing_variants_serialize_to_camel_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&Easing::Linear).unwrap(),
            r#""linear""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseIn).unwrap(),
            r#""easeIn""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseOut).unwrap(),
            r#""easeOut""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseInOut).unwrap(),
            r#""easeInOut""#
        );
        // And each parses back from its literal.
        let parsed: Easing = serde_json::from_str(r#""easeInOut""#).unwrap();
        assert_eq!(parsed, Easing::EaseInOut);
    }

    #[test]
    fn style_range_less_text_round_trips_byte_identically() {
        // A pre-E `text` widget carrying no `styleRanges` must keep its EXACT wire
        // form: the new field skip-serializes when absent (default), so a
        // styleRanges-less descriptor is byte-identical across a round-trip. This
        // is the locked-wire-format guarantee for Goal E's additive field.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("styleRanges"),
            "absent styleRanges emits no key"
        );
    }

    #[test]
    fn style_range_less_panel_round_trips_byte_identically() {
        // The panel-side twin: a styleRanges-less panel keeps its pre-E wire form.
        let json = r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":null}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("styleRanges"),
            "absent styleRanges emits no key"
        );
    }

    #[test]
    fn capture_mode_bearing_tree_round_trips_in_camel_case() {
        // A `captureMode: "capture"` envelope round-trips byte-for-byte. Field
        // order: anchor, offset, root, then captureMode (declaration order).
        let json = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0},"captureMode":"capture"}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(tree.capture_mode, CaptureMode::Capture);
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn capture_mode_absent_round_trips_byte_identically_as_passthrough() {
        // A pre-F descriptor with no `captureMode` key deserializes to the default
        // `Passthrough` and re-serializes WITHOUT the key (skip_serializing_if), so
        // the wire form stays byte-identical to the pre-F shape.
        let json =
            r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Passthrough,
            "absent captureMode defaults to passthrough",
        );
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("captureMode"),
            "passthrough captureMode emits no key",
        );
    }

    #[test]
    fn capture_mode_serializes_to_camel_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&CaptureMode::Capture).unwrap(),
            r#""capture""#
        );
        assert_eq!(
            serde_json::to_string(&CaptureMode::Passthrough).unwrap(),
            r#""passthrough""#
        );
    }

    #[test]
    fn text_with_style_ranges_round_trips_in_camel_case() {
        // A `text` widget carrying `styleRanges` keeps its camelCase wire form
        // byte-for-byte. Field order: content, fontSize, color, bind, then
        // styleRanges { max, entries: [{ upTo, color }, { color }] }.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"},"styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    // --- M13 Goal F, Task 3: focus wire-form additive fields ---

    #[test]
    fn focus_field_less_widget_round_trips_byte_identically() {
        // A pre-F widget carrying none of the new focus fields (`id`,
        // `focusNeighbors`, `focus`, `restoreOnReturn`) keeps its EXACT wire form:
        // every new field skip-serializes when absent/default, so the descriptor is
        // byte-identical across a round-trip. The locked-wire guarantee for Task 3.
        let json = r#"{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hi","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        for key in ["\"id\"", "focusNeighbors", "\"focus\"", "restoreOnReturn"] {
            assert!(!reserialized.contains(key), "absent {key} emits no key");
        }
    }

    #[test]
    fn container_focus_policy_shorthand_and_detailed_round_trip() {
        // The `focus` field is an untagged union: a bare string shorthand
        // (`"linear"`) or a detailed object. Both round-trip byte-identically.
        let shorthand = r#"{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","focus":"linear","children":[]}"#;
        let w: Widget = serde_json::from_str(shorthand).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), shorthand);

        // Detailed form with wrap:false and a repeat policy. `wrap` skip-serializes
        // only when true (its default), so an authored `false` is emitted.
        let detailed = r#"{"kind":"grid","gap":0.0,"padding":0.0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300.0,"intervalMs":80.0}},"children":[]}"#;
        let w: Widget = serde_json::from_str(detailed).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), detailed);
    }

    #[test]
    fn focus_policy_accessors_resolve_kind_wrap_and_repeat() {
        // Shorthand: linear kind, wrap defaults on, no repeat.
        let sh: FocusPolicy = serde_json::from_str(r#""linear""#).unwrap();
        assert_eq!(sh.kind(), FocusKind::Linear);
        assert!(sh.wrap());
        assert!(sh.repeat().is_none());

        // Detailed: spatial, wrap off, repeat carried through.
        let det: FocusPolicy = serde_json::from_str(
            r#"{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":250.0,"intervalMs":60.0}}"#,
        )
        .unwrap();
        assert_eq!(det.kind(), FocusKind::Spatial);
        assert!(!det.wrap());
        let r = det.repeat().expect("repeat carried");
        assert_eq!(r.initial_delay_ms, 250.0);
        assert_eq!(r.interval_ms, 60.0);
    }

    #[test]
    fn node_id_and_focus_neighbors_round_trip() {
        // An authored `id` and partial `focusNeighbors` keep their camelCase wire
        // form; the unset neighbor directions omit their keys.
        let json = r#"{"kind":"text","content":"A","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"id":"btnA","focusNeighbors":{"down":"btnB","right":"btnC"}}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);
    }

    #[test]
    fn anchored_tree_initial_focus_and_restore_on_return_round_trip() {
        // `initialFocus` lives on the envelope beside `captureMode`;
        // `restoreOnReturn` on the container. Both round-trip byte-identically.
        let json = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","restoreOnReturn":true,"children":[]},"captureMode":"capture","initialFocus":"btnA"}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tree.initial_focus.as_deref(), Some("btnA"));
        assert_eq!(serde_json::to_string(&tree).unwrap(), json);
    }

    // --- M13 Text-Entry, Task 3: text-entry target envelope field ---

    #[test]
    fn anchored_tree_text_entry_target_round_trips_beside_capture_mode() {
        // `textEntryTarget` lives on the envelope beside `captureMode` /
        // `initialFocus`; it round-trips byte-identically. Field order matches the
        // struct declaration: anchor, offset, root, captureMode, initialFocus,
        // textEntryTarget.
        let json = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0},"captureMode":"capture","textEntryTarget":"ui.textEntry"}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(tree.text_entry_target.as_deref(), Some("ui.textEntry"));
        assert_eq!(serde_json::to_string(&tree).unwrap(), json);
    }

    #[test]
    fn anchored_tree_text_entry_target_absent_omits_the_key() {
        // A tree without text entry omits `textEntryTarget` entirely
        // (skip_serializing_if), so a non-text-entry tree round-trips
        // byte-identically with its pre-text-entry wire form.
        let json =
            r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(tree.text_entry_target, None);
        let reserialized = serde_json::to_string(&tree).unwrap();
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("textEntryTarget"),
            "absent textEntryTarget emits no key"
        );
    }

    // --- M13 Goal F, Task 4: interactive widgets ---

    #[test]
    fn button_round_trips_with_on_press_in_camel_case() {
        // A `button` carrying id/label/onPress keeps its camelCase wire form.
        // Field order: kind, id, label, onPress (declaration order).
        let json = r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert!(matches!(widget, Widget::Button(_)));
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn button_repeat_on_hold_round_trips_and_absent_flag_is_byte_identical() {
        // M13 Text-Entry, Task 2: the opt-in `repeatOnHold` flag carries the same
        // `{ initialDelayMs, intervalMs }` wire shape as a container's nav `repeat`.
        // A flagged button round-trips byte-identically; field order is kind, id,
        // label, onPress, then repeatOnHold (declaration order).
        let flagged = r#"{"kind":"button","id":"bksp","label":"DEL","onPress":"backspace","repeatOnHold":{"initialDelayMs":400.0,"intervalMs":60.0}}"#;
        let widget: Widget = serde_json::from_str(flagged).expect("must deserialize");
        match &widget {
            Widget::Button(b) => {
                let p = b.repeat_on_hold.expect("flag parsed");
                assert_eq!(p.initial_delay_ms, 400.0);
                assert_eq!(p.interval_ms, 60.0);
            }
            _ => panic!("expected button"),
        }
        assert_eq!(serde_json::to_string(&widget).unwrap(), flagged);

        // A flag-less button keeps its pre-text-entry wire form byte-identical: the
        // additive field skip-serializes when absent (the locked-wire guarantee).
        let plain = r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).unwrap();
        assert_eq!(reserialized, plain);
        assert!(
            !reserialized.contains("repeatOnHold"),
            "absent repeatOnHold emits no key"
        );
    }

    #[test]
    fn button_with_focus_neighbors_round_trips_and_capture_less_omits_keys() {
        let json = r#"{"kind":"button","id":"a","label":"A","onPress":"fa","focusNeighbors":{"down":"b"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
        // A neighborless button omits the focusNeighbors key entirely.
        let plain = r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), plain);
    }

    #[test]
    fn slider_round_trips_with_captures_nav_array() {
        // `capturesNav` is an ARRAY of nav wire names, not a bool. The slider
        // round-trips byte-identically. Field order: kind, id, label, bind, min,
        // max, step, capturesNav.
        let json = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1,"capturesNav":["nav.left","nav.right"]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        match &widget {
            Widget::Slider(s) => {
                assert_eq!(s.captures_nav, vec!["nav.left", "nav.right"]);
                assert_eq!(s.min, 0.0);
                assert_eq!(s.max, 1.0);
                assert_eq!(s.step, 0.1);
            }
            _ => panic!("expected slider"),
        }
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn slider_omits_empty_captures_nav_and_supports_bind_tween() {
        // No capturesNav and no tween: both keys omitted.
        let plain = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).unwrap();
        assert_eq!(reserialized, plain);
        assert!(!reserialized.contains("capturesNav"));
        assert!(!reserialized.contains("tween"));

        // A bind tween (number shape, TextTween) round-trips.
        let tween = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master","tween":{"durationMs":120.0,"easing":"easeOut"}},"min":0.0,"max":1.0,"step":0.1}"#;
        let widget: Widget = serde_json::from_str(tween).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), tween);
    }

    #[test]
    fn bar_round_trips_with_max_fill_background() {
        // A `bar` binding `player.health` with max/fill/background. Field order:
        // kind, bind, max, fill, background.
        let json = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":[0.0,1.0,0.0,1.0],"background":[0.1,0.1,0.1,1.0]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert!(matches!(widget, Widget::Bar(_)));
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn bar_round_trips_with_style_ranges_and_token_colors() {
        // A bar may use theme-token color slots and a styleRanges map. Both
        // round-trip byte-identically; absent id/styleRanges omit their keys.
        let json = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":"ok","background":"panel.default","styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);

        let plain = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":[0.0,1.0,0.0,1.0],"background":[0.1,0.1,0.1,1.0]}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).unwrap();
        assert_eq!(reserialized, plain);
        assert!(!reserialized.contains("styleRanges"));
        assert!(!reserialized.contains("\"id\""));
    }

    // --- M13 G1a, Task 3: SDK widget/layout factory output validation ---
    //
    // The Task 5 deserialization bridge does not exist yet, so the SDK factories
    // (sdk/lib/ui/widgets.{ts,luau}, layout.{ts,luau}) are validated here by
    // round-tripping their EXACT emitted JSON through serde: each string below is
    // the literal output of the matching TS factory call (captured by running the
    // factories under bun). JS emits bare integers (`1`, not `1.0`) for whole
    // floats and OMITS an absent panel `border`; deserializing into the `Widget`
    // variant and re-serializing must yield the canonical wire form. This proves
    // factory output is a valid descriptor and resolves to the locked wire shape.
    //
    // Each tuple is (factory-emitted JSON, canonical re-serialized JSON).
    #[test]
    fn sdk_factory_output_round_trips_to_canonical_wire_form() {
        let cases: &[(&str, &str)] = &[
            // Text() with defaults (fontSize 12, white color).
            (
                r#"{"kind":"text","content":"hello","fontSize":12,"color":[1,1,1,1]}"#,
                r#"{"kind":"text","content":"hello","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#,
            ),
            // Text() with a bind carrying slot + format.
            (
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health","format":"HP {}"}}"#,
                r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","format":"HP {}"}}"#,
            ),
            // Text() bind with a tween (number-shape from).
            (
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health","tween":{"durationMs":1200,"easing":"easeOut","from":0}}}"#,
                r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","tween":{"durationMs":1200.0,"easing":"easeOut","from":0.0}}}"#,
            ),
            // Text() with styleRanges (token colors).
            (
                r#"{"kind":"text","content":"0","fontSize":18,"color":[1,1,1,1],"bind":{"slot":"player.health"},"styleRanges":{"max":100,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#,
                r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"},"styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#,
            ),
            // Panel() with a border.
            (
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1],"border":{"texture":"ui/frame","slice":[8,8,8,8],"tint":[1,1,1,1]}}"#,
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":{"texture":"ui/frame","slice":[8.0,8.0,8.0,8.0],"tint":[1.0,1.0,1.0,1.0]}}"#,
            ),
            // Panel() with NO border: the factory omits the key; serde defaults it
            // to None and re-serializes as `border:null` (the canonical form).
            (
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1]}"#,
                r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":null}"#,
            ),
            // Panel() bind with a color-shape tween `from`.
            (
                r#"{"kind":"panel","fill":[0,0,0,1],"bind":{"slot":"intro.flashColor","tween":{"durationMs":300,"easing":"linear","from":[1,0,0,1]}}}"#,
                r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor","tween":{"durationMs":300.0,"easing":"linear","from":[1.0,0.0,0.0,1.0]}}}"#,
            ),
            // Image() — no bind.
            (
                r#"{"kind":"image","asset":"ui/logo"}"#,
                r#"{"kind":"image","asset":"ui/logo"}"#,
            ),
            // Spacer() with explicit flexGrow.
            (
                r#"{"kind":"spacer","flexGrow":1}"#,
                r#"{"kind":"spacer","flexGrow":1.0}"#,
            ),
            // Button() with a bare-name onPress.
            (
                r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#,
                r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#,
            ),
            // Button() with a reaction-handle onPress (factory read `.name` → "fa").
            (
                r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#,
                r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#,
            ),
            // Slider() with capturesNav.
            (
                r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0,"max":1,"step":0.1,"capturesNav":["nav.left","nav.right"]}"#,
                r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1,"capturesNav":["nav.left","nav.right"]}"#,
            ),
            // Bar() plain.
            (
                r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100,"fill":[0,1,0,1],"background":[0.1,0.1,0.1,1]}"#,
                r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":[0.0,1.0,0.0,1.0],"background":[0.1,0.1,0.1,1.0]}"#,
            ),
            // VStack() with one child.
            (
                r#"{"kind":"vstack","gap":4,"padding":8,"align":"start","children":[{"kind":"text","content":"hi","fontSize":12,"color":[1,1,1,1]}]}"#,
                r#"{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hi","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}]}"#,
            ),
            // Grid() with one child.
            (
                r#"{"kind":"grid","gap":1,"padding":3,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon"}]}"#,
                r#"{"kind":"grid","gap":1.0,"padding":3.0,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon"}]}"#,
            ),
            // Grid() with a detailed focus policy (wrap:false + repeat) and defaults.
            (
                r#"{"kind":"grid","gap":0,"padding":0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300,"intervalMs":80}},"children":[]}"#,
                r#"{"kind":"grid","gap":0.0,"padding":0.0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300.0,"intervalMs":80.0}},"children":[]}"#,
            ),
        ];

        for (emitted, canonical) in cases {
            let widget: Widget = serde_json::from_str(emitted)
                .unwrap_or_else(|e| panic!("factory output must deserialize: {emitted}\n{e}"));
            let reserialized = serde_json::to_string(&widget).expect("must serialize");
            assert_eq!(
                &reserialized, canonical,
                "factory output did not resolve to the canonical wire form:\nemitted:    {emitted}\nexpected:   {canonical}\nactual:     {reserialized}"
            );
        }
    }

    // --- M13 G1a, Task 4: SDK Tree(...) envelope factory output validation ---
    //
    // The Task 5 deserialization bridge does not exist yet, so the SDK `Tree(...)`
    // factory (sdk/lib/ui/tree.{ts,luau}) is validated here by round-tripping its
    // EXACT emitted JSON through the `AnchoredTree` serde model: each string below
    // is the literal output of the matching TS factory call (captured by running
    // `tree.ts` under bun). JS emits bare integers (`0`, not `0.0`); deserializing
    // into `AnchoredTree` and re-serializing must yield the canonical wire form,
    // proving the envelope is a valid descriptor that resolves to the locked shape.
    //
    // Each tuple is (factory-emitted JSON, canonical re-serialized JSON).
    #[test]
    fn sdk_tree_factory_output_round_trips_to_canonical_wire_form() {
        let cases: &[(&str, &str)] = &[
            // Tree() with captureMode OMITTED: the factory drops the key; serde
            // defaults it to Passthrough and skip-serializes it back out.
            (
                r#"{"anchor":"center","offset":[0,0],"root":{"kind":"spacer","flexGrow":1}}"#,
                r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#,
            ),
            // Tree() with captureMode "passthrough": the factory drops the key too
            // (passthrough round-trips to omission), identical to the omitted case.
            (
                r#"{"anchor":"center","offset":[0,0],"root":{"kind":"spacer","flexGrow":1}}"#,
                r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#,
            ),
            // Tree() with captureMode "capture": the factory emits the key; it
            // deserializes to CaptureMode::Capture and re-serializes with the key.
            (
                r#"{"anchor":"center","offset":[0,0],"root":{"kind":"spacer","flexGrow":1},"captureMode":"capture"}"#,
                r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0},"captureMode":"capture"}"#,
            ),
            // Tree() with capture + initialFocus + textEntryTarget, non-zero offset.
            (
                r#"{"anchor":"topLeft","offset":[10,-20],"root":{"kind":"spacer","flexGrow":1},"captureMode":"capture","initialFocus":"btnA","textEntryTarget":"ui.textEntry"}"#,
                r#"{"anchor":"topLeft","offset":[10.0,-20.0],"root":{"kind":"spacer","flexGrow":1.0},"captureMode":"capture","initialFocus":"btnA","textEntryTarget":"ui.textEntry"}"#,
            ),
        ];

        for (emitted, canonical) in cases {
            let tree: AnchoredTree = serde_json::from_str(emitted)
                .unwrap_or_else(|e| panic!("Tree() output must deserialize: {emitted}\n{e}"));
            let reserialized = serde_json::to_string(&tree).expect("must serialize");
            assert_eq!(
                &reserialized, canonical,
                "Tree() output did not resolve to the canonical wire form:\nemitted:    {emitted}\nexpected:   {canonical}\nactual:     {reserialized}"
            );
        }

        // The omitted and explicit-"passthrough" cases must both deserialize to
        // the default Passthrough and emit NO captureMode key.
        let passthrough: AnchoredTree = serde_json::from_str(
            r#"{"anchor":"center","offset":[0,0],"root":{"kind":"spacer","flexGrow":1}}"#,
        )
        .expect("deserialize");
        assert_eq!(passthrough.capture_mode, CaptureMode::Passthrough);
        assert!(
            !serde_json::to_string(&passthrough)
                .unwrap()
                .contains("captureMode")
        );

        // The explicit-"capture" case deserializes to Capture.
        let capture: AnchoredTree = serde_json::from_str(
            r#"{"anchor":"center","offset":[0,0],"root":{"kind":"spacer","flexGrow":1},"captureMode":"capture"}"#,
        )
        .expect("deserialize");
        assert_eq!(capture.capture_mode, CaptureMode::Capture);
    }

    // --- M13 G1b, Task 5: localState + `{ local }` bind ---------------------

    #[test]
    fn local_state_less_container_round_trips_without_the_key() {
        // The new `localState` field skip-serializes when absent, so a pre-G1b
        // container is byte-identical across a round-trip (absent → no key).
        let json = r#"{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("localState"),
            "absent localState emits no key"
        );
    }

    #[test]
    fn container_with_local_state_round_trips_byte_identically() {
        // A container declaring a `localState` scope + cells keeps its wire form.
        // Field order: gap, padding, align, localState (scope, cells), children.
        // `cells` is a BTreeMap, so its keys serialize in stable sorted order.
        let json = r#"{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","localState":{"scope":"counter","cells":{"count":0.0,"flash":[1.0,0.0,0.0,1.0]}},"children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bind_slot_and_local_alternatives_each_round_trip_in_their_own_form() {
        // The bind source is an untagged `{ slot }` vs `{ local }` alternative: a
        // store binding carries `slot`, a presentation-cell binding carries
        // `local`, and each re-serializes byte-identically to the form authored.
        let slot = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"}}"#;
        let w: Widget = serde_json::from_str(slot).expect("slot bind deserializes");
        assert_eq!(serde_json::to_string(&w).unwrap(), slot);

        let local = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"local":"count"}}"#;
        let w: Widget = serde_json::from_str(local).expect("local bind deserializes");
        assert_eq!(serde_json::to_string(&w).unwrap(), local);
    }

    #[test]
    fn local_bind_parses_into_the_local_source_variant() {
        // Pin the variant the disjoint wire forms land on.
        let bind: TextBind = serde_json::from_str(r#"{"local":"count"}"#).unwrap();
        assert_eq!(
            bind.source,
            BindSource::Local {
                local: "count".into()
            }
        );
        let bind: TextBind = serde_json::from_str(r#"{"slot":"a.b"}"#).unwrap();
        assert_eq!(bind.source, BindSource::Slot { slot: "a.b".into() });
    }

    #[test]
    fn panel_local_bind_with_tween_round_trips() {
        // A `{ local }` panel bind carrying a tween keeps its wire form — the
        // flattened `local` source sits beside `tween`.
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"local":"flash","tween":{"durationMs":150.0,"easing":"linear"}}}"#;
        let w: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);
    }

    // --- M13 G2: descriptor vocabulary (predicate / a11y / announce) --------

    #[test]
    fn predicate_round_trips_with_and_without_equals() {
        // A predicate flattens its `{ slot }`/`{ local }` source beside an optional
        // `equals` comparand. With `equals` present and absent, each round-trips
        // byte-identically; an absent `equals` omits the key.
        let with_eq = r#"{"slot":"hud.tab","equals":"stats"}"#;
        let p: Predicate = serde_json::from_str(with_eq).expect("deserialize");
        assert_eq!(serde_json::to_string(&p).unwrap(), with_eq);

        let truthy = r#"{"local":"open"}"#;
        let p: Predicate = serde_json::from_str(truthy).expect("deserialize");
        assert_eq!(serde_json::to_string(&p).unwrap(), truthy);
        assert!(p.equals.is_none());
    }

    #[test]
    fn predicate_equals_accepts_scalars_and_rejects_array() {
        // `equals` admits number / bool / string only.
        let n: Predicate = serde_json::from_str(r#"{"slot":"a.b","equals":5}"#).unwrap();
        assert_eq!(n.equals, Some(PredicateValue::Number(5.0)));
        let b: Predicate = serde_json::from_str(r#"{"slot":"a.b","equals":true}"#).unwrap();
        assert_eq!(b.equals, Some(PredicateValue::Boolean(true)));
        let s: Predicate = serde_json::from_str(r#"{"slot":"a.b","equals":"on"}"#).unwrap();
        assert_eq!(s.equals, Some(PredicateValue::String("on".into())));

        // An rgba/array comparand is a load-time error: it matches no scalar
        // variant of the untagged `PredicateValue`, so serde rejects it.
        let arr: Result<Predicate, _> =
            serde_json::from_str(r#"{"slot":"a.b","equals":[1.0,0.0,0.0,1.0]}"#);
        assert!(
            arr.is_err(),
            "an rgba/array `equals` comparand must be a load-time error"
        );
    }

    #[test]
    fn button_with_selected_checked_bind_disabled_round_trips() {
        // The G2 button reactive-state fields (selected/checked predicates, a
        // styleRanges value `bind`, and `disabled`) round-trip byte-identically.
        // Field order: id, label, onPress, selected, checked, bind, styleRanges,
        // disabled.
        let json = r#"{"kind":"button","id":"tab1","label":"Stats","onPress":"openStats","selected":{"slot":"hud.tab","equals":"stats"},"checked":{"local":"on"},"bind":{"slot":"hud.charge"},"styleRanges":{"max":100.0,"entries":[{"color":"ok"}]},"disabled":true}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);
    }

    #[test]
    fn button_g2_fields_absent_round_trips_byte_identically() {
        // A pre-G2 button (no selected/checked/bind/styleRanges/disabled/
        // visibleWhen/role) keeps its EXACT wire form: every new field skip-
        // serializes when absent/false. label stays present (one-of name).
        let json = r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        let re = serde_json::to_string(&w).unwrap();
        assert_eq!(re, json);
        for key in [
            "selected",
            "checked",
            "\"bind\"",
            "styleRanges",
            "disabled",
            "visibleWhen",
            "\"role\"",
        ] {
            assert!(!re.contains(key), "absent {key} emits no key");
        }
    }

    #[test]
    fn button_labelled_by_round_trips_with_label_omitted() {
        // A `labelledBy` button omits the inline `label` key entirely (label is now
        // Option, skip-serialized when absent).
        let json = r#"{"kind":"button","id":"x","labelledBy":"xLabel","onPress":"go"}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        let re = serde_json::to_string(&w).unwrap();
        assert_eq!(re, json);
        assert!(
            !re.contains("\"label\""),
            "labelledBy button omits inline label"
        );
    }

    #[test]
    fn visible_when_round_trips_on_every_kind_and_absent_omits_the_key() {
        // `visibleWhen` rides every widget variant. A predicate-bearing widget
        // round-trips byte-identically; absent it omits the key (locked-wire).
        let with = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"visibleWhen":{"slot":"hud.show"}}"#;
        let w: Widget = serde_json::from_str(with).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), with);

        let without = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#;
        let w: Widget = serde_json::from_str(without).expect("deserialize");
        let re = serde_json::to_string(&w).unwrap();
        assert_eq!(re, without);
        assert!(
            !re.contains("visibleWhen"),
            "absent visibleWhen emits no key"
        );
    }

    #[test]
    fn role_override_round_trips_and_absent_omits_the_key() {
        // An authored `role` override round-trips in its camelCase wire literal;
        // absent omits the key (the implicit role is runtime-only).
        let json = r#"{"kind":"button","id":"t","label":"Tab","onPress":"go","role":"tab"}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);

        let plain = r#"{"kind":"button","id":"t","label":"Tab","onPress":"go"}"#;
        let w: Widget = serde_json::from_str(plain).expect("deserialize");
        assert!(!serde_json::to_string(&w).unwrap().contains("\"role\""));
    }

    #[test]
    fn role_variants_serialize_to_camel_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&Role::Progressbar).unwrap(),
            r#""progressbar""#
        );
        assert_eq!(
            serde_json::to_string(&Role::Tablist).unwrap(),
            r#""tablist""#
        );
        assert_eq!(serde_json::to_string(&Role::None).unwrap(), r#""none""#);
        let parsed: Role = serde_json::from_str(r#""checkbox""#).unwrap();
        assert_eq!(parsed, Role::Checkbox);
    }

    #[test]
    fn image_label_and_decorative_round_trip() {
        // An image carries label OR decorative; each round-trips byte-identically.
        let named = r#"{"kind":"image","asset":"ui/portrait","label":"Hero portrait"}"#;
        let w: Widget = serde_json::from_str(named).expect("deserialize");
        let re = serde_json::to_string(&w).unwrap();
        assert_eq!(re, named);
        // `decorative` skip-serializes when false.
        assert!(
            !re.contains("decorative"),
            "named image omits decorative:false"
        );

        let deco = r#"{"kind":"image","asset":"ui/logo","decorative":true}"#;
        let w: Widget = serde_json::from_str(deco).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), deco);
    }

    #[test]
    fn announce_polite_round_trips_byte_identically_omitting_priority() {
        // A polite announce (default) omits the `priority` key entirely.
        let json = r#"{"kind":"announce","text":"Saved"}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        let re = serde_json::to_string(&w).unwrap();
        assert_eq!(re, json);
        assert!(!re.contains("priority"), "polite priority emits no key");
        assert!(matches!(w, Widget::Announce(_)));
    }

    #[test]
    fn announce_assertive_round_trips_with_priority_present() {
        let json = r#"{"kind":"announce","text":"Alert","priority":"assertive"}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);
    }

    #[test]
    fn anchored_tree_accessible_name_and_role_round_trip_and_absent_is_byte_identical() {
        // The envelope carries optional `accessibleName` + `role` (Option::is_none
        // skip). Present they round-trip; absent the tree is byte-identical to its
        // pre-G2 wire (no new keys).
        let with = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0},"accessibleName":"Pause menu","role":"group"}"#;
        let tree: AnchoredTree = serde_json::from_str(with).expect("deserialize");
        assert_eq!(tree.accessible_name.as_deref(), Some("Pause menu"));
        assert_eq!(tree.role, Some(Role::Group));
        assert_eq!(serde_json::to_string(&tree).unwrap(), with);

        let without =
            r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#;
        let tree: AnchoredTree = serde_json::from_str(without).expect("deserialize");
        let re = serde_json::to_string(&tree).unwrap();
        assert_eq!(re, without);
        assert!(!re.contains("accessibleName") && !re.contains("\"role\""));
    }
}
