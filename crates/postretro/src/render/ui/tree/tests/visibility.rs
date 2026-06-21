// Reactive visibleWhen show/hide across draw and focus.

use super::common::*;
/// A focusable text leaf carrying a `visibleWhen` predicate over `slot`
/// (boolean truthiness). Drawn (one glyph run) and focusable when shown.
fn text_id_visible(content: &str, id: &str, slot: &str) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size: 20.0,
        color: ColorValue::Literal([1.0; 4]),
        font: None,
        id: Some(id.to_string()),
        focus_neighbors: crate::render::ui::descriptor::FocusNeighbors::default(),
        bind: None,
        style_ranges: None,
        visible_when: Some(pred(slot, None)),
        role: None,
    })
}

/// A linear-focus vstack wrapping `children` (each its own focusable leaf).
fn focus_vstack(children: Vec<Widget>) -> Widget {
    use crate::render::ui::descriptor::{FocusKind, FocusPolicy};
    Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(10.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: crate::render::ui::descriptor::FocusNeighbors::default(),
        focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children,
    })
}

fn visibility_slots(slot: &str, value: bool) -> HashMap<String, SlotValue> {
    let mut m = HashMap::new();
    m.insert(slot.to_string(), SlotValue::Boolean(value));
    m
}

#[test]
fn visible_when_false_hides_subtree_from_draw_and_focus() {
    // A false `visibleWhen` sets the node Display::None: zero glyph runs for the
    // hidden leaf, its focusable drops out of the rect list, and it is not a
    // candidate for the declared initial focus.
    let root = focus_vstack(vec![
        text_id("Always", "always"),
        text_id_visible("Maybe", "maybe", "hud.advanced"),
    ]);
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        // The hidden node is the declared initial focus — it must NOT be a
        // candidate while hidden (its id is absent from the rect list).
        initial_focus: Some("maybe".to_string()),
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    // Predicate false → "maybe" hidden.
    let hidden = visibility_slots("hud.advanced", false);
    let draw = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &hidden,
        &no_cells(),
        0.0,
    );
    let focus = ui.export_focus_rects(&tree, [1280, 720], &hidden, &no_cells());

    assert!(
        draw.texts.iter().all(|t| t.content != "Maybe"),
        "hidden leaf draws zero glyph runs, got: {:?}",
        draw.texts.iter().map(|t| &t.content).collect::<Vec<_>>(),
    );
    assert!(
        draw.texts.iter().any(|t| t.content == "Always"),
        "visible sibling still draws",
    );
    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["always"], "hidden focusable is unreachable");
    assert!(
        !focus.rects.iter().any(|r| r.id == "maybe"),
        "hidden node is not an initial-focus candidate",
    );
}

#[test]
fn visible_when_true_restores_draw_and_focus() {
    // Round-trip the same tree with the predicate true: the previously hidden
    // node draws its glyph run and rejoins the focus rect list.
    let root = focus_vstack(vec![
        text_id("Always", "always"),
        text_id_visible("Maybe", "maybe", "hud.advanced"),
    ]);
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: Some("maybe".to_string()),
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    // Frame 1: hidden.
    let hidden = visibility_slots("hud.advanced", false);
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &hidden,
        &no_cells(),
        0.0,
    );

    // Frame 2: predicate true → restored.
    let shown = visibility_slots("hud.advanced", true);
    let draw =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &shown, &no_cells(), 0.0);
    let focus = ui.export_focus_rects(&tree, [1280, 720], &shown, &no_cells());

    assert!(
        draw.texts.iter().any(|t| t.content == "Maybe"),
        "shown leaf draws its glyph run again",
    );
    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["always", "maybe"], "shown focusable rejoins the list");
}

#[test]
fn visible_when_resolved_change_marks_dirty_and_reexports() {
    // A change in the predicate's resolved value relays out (marks dirty) and
    // the re-exported focus rect list reflects the new visibility; an unchanged
    // resolved value does NOT relayout (targeted invalidation).
    let root = focus_vstack(vec![text_id_visible("Maybe", "maybe", "hud.advanced")]);
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    // Frame 1: hidden (false). First frame always computes once.
    let hidden = visibility_slots("hud.advanced", false);
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &hidden,
        &no_cells(),
        0.0,
    );
    let after_first = ui.recompute_count();
    let focus_hidden = ui.export_focus_rects(&tree, [1280, 720], &hidden, &no_cells());
    assert!(
        focus_hidden.rects.is_empty(),
        "hidden node exports no focus rect",
    );

    // Frame 2: same resolved value (still false) — no flip, no relayout.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &hidden,
        &no_cells(),
        0.0,
    );
    assert_eq!(
        ui.recompute_count(),
        after_first,
        "an unchanged predicate value must not relayout",
    );

    // Frame 3: resolved value flips to true — marks dirty, relays out.
    let shown = visibility_slots("hud.advanced", true);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &shown, &no_cells(), 0.0);
    assert!(
        ui.recompute_count() > after_first,
        "a resolved-value flip marks layout dirty and relays out",
    );
    let focus_shown = ui.export_focus_rects(&tree, [1280, 720], &shown, &no_cells());
    let ids: Vec<&str> = focus_shown.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["maybe"], "re-export reflects the now-visible node");
}

#[test]
fn grid_container_visible_when_true_restores_display_grid_not_flex() {
    // A grid container carrying `visibleWhen` must restore to `Display::Grid`
    // on a hide→show flip — not `Display::Flex`. `VisibilityState::visible_display`
    // captures the authored display at build time so the round-trip is correct.
    // Verified by checking that the grid's children still lay out in columns
    // (not stacked as a flex column) after restoration.
    use crate::render::ui::descriptor::GridWidget;
    let cell = || {
        Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 10.0,
            color: ColorValue::Literal([1.0; 4]),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        })
    };
    let grid = Widget::Grid(GridWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        cols: 2,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        visible_when: Some(pred("grid.show", None)),
        role: None,
        children: vec![cell(), cell(), cell(), cell()],
    });
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: grid,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    // Frame 1: grid hidden.
    let hidden = visibility_slots("grid.show", false);
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &hidden,
        &no_cells(),
        0.0,
    );

    // Frame 2: grid shown — `visible_display` must restore `Display::Grid`.
    let shown = visibility_slots("grid.show", true);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &shown, &no_cells(), 0.0);

    // After restoration the four children must span two columns: children 0
    // and 1 share a row (same y, different x), and child 2 wraps to a new row.
    let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
    assert_eq!(
        children.len(),
        4,
        "grid has four children after restoration"
    );
    let layout = |n: NodeId| ui.taffy.layout(n).unwrap();
    let y0 = layout(children[0]).location.y;
    let y1 = layout(children[1]).location.y;
    let y2 = layout(children[2]).location.y;
    assert!(
        approx(y0, y1),
        "after restoration children 0 and 1 share a row (Display::Grid), got y0={y0} y1={y1}",
    );
    assert!(
        y2 > y0,
        "after restoration child 2 wraps to a lower row, got y0={y0} y2={y2}",
    );
}

#[test]
fn visible_when_local_cell_hides_and_shows_node() {
    // A `visibleWhen` predicate sourced from a `{ local }` cell (not `{ slot }`)
    // hides the node when the cell is false and shows it when the cell is true.
    // All existing visibility tests use `{ slot }`; this exercises the `{ local }`
    // path through `harvest_visibility` and `resolve_bindings`.
    let scoped_tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: Some(LocalState {
                scope: "hud".to_string(),
                cells: Default::default(),
            }),
            // The container's own `visibleWhen` references a PARENT scope cell
            // (a sibling cell is the intended pattern — the own-scope caveat
            // documented in `harvest_visibility` means using one's own cell here
            // would silently resolve 0.0). We use a plain text child instead.
            visible_when: None,
            role: None,
            children: vec![Widget::Text(TextWidget {
                content: "Cell-gated".into(),
                font_size: 18.0,
                color: ColorValue::Literal([1.0; 4]),
                font: None,
                id: None,
                focus_neighbors: Default::default(),
                bind: None,
                style_ranges: None,
                visible_when: Some(Predicate {
                    source: BindSource::Local {
                        local: "show".to_string(),
                    },
                    equals: None,
                }),
                role: None,
            })],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };

    let mut ui = UiTree::from_descriptor(&scoped_tree, &theme());
    let mut fs = font_system();

    // Cell false → text hidden.
    let mut hidden_cells = CellValues::new();
    hidden_cells.insert(
        ("hud".to_string(), "show".to_string()),
        SlotValue::Boolean(false),
    );
    let draw = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &no_slots(),
        &hidden_cells,
        0.0,
    );
    assert!(
        draw.texts.iter().all(|t| t.content != "Cell-gated"),
        "cell=false hides the node, got: {:?}",
        draw.texts.iter().map(|t| &t.content).collect::<Vec<_>>(),
    );

    // Cell true → text shown.
    let mut shown_cells = CellValues::new();
    shown_cells.insert(
        ("hud".to_string(), "show".to_string()),
        SlotValue::Boolean(true),
    );
    let draw = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &no_slots(),
        &shown_cells,
        0.0,
    );
    assert!(
        draw.texts.iter().any(|t| t.content == "Cell-gated"),
        "cell=true shows the node again, got: {:?}",
        draw.texts.iter().map(|t| &t.content).collect::<Vec<_>>(),
    );
}
