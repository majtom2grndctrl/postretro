// Hard-gate CPU assertion for the gameplay UI path (Task 6).
//
// The renderer's gameplay path (`render_frame_indirect` in `render/mod.rs`) lays
// the snapshot's descriptor tree out and decides whether to open the UI pass:
//   - no tree, or a tree that lays out empty -> EARLY-OUT, no `begin_render_pass`
//   - a tree that produces drawable output -> open the pass and record the batches
//
// This test pins that decision purely on the CPU by reproducing the same layout
// (`UiTree::build_draw_data`) + early-out predicate (`UiDrawData::is_empty`) the
// renderer uses, with a headless `FontSystem` for text measurement. It feeds a
// FIXTURE tree (not a real screen) per the plan, and covers the full new vocab —
// flex distribution, grid placement, measured text, anchor-against-letterbox, and
// integer pixel snapping — by asserting a representative composite tree produces a
// non-empty, well-snapped draw list. No GPU adapter, no wgpu call.
//
// CRITICAL: the early-out is gameplay-path-only. The splash path opens the pass
// unconditionally for its frame-0 black clear (`record_splash_ui`), so this gate
// asserts ONLY the gameplay predicate — it does not touch the splash clear.
//
// See: context/plans/in-progress/M13--descriptor-tree-layout (Task 6 gate; AC:
// "an empty tree early-outs the UI pass (no begin_render_pass)").

use super::UiReadSnapshot;
use super::descriptor::{
    Align, AnchoredTree, ContainerWidget, GridWidget, ImageWidget, TextWidget, Widget,
};
use super::layout::Anchor;
use super::tree::{ImageSizes, UiDrawData, UiTree};

const EPS: f32 = 1e-3;

fn font_system() -> glyphon::FontSystem {
    super::text::build_font_system()
}

/// Natural reference sizes for the fixture's image assets, threaded into the
/// measure seam so the grid's icon images size content-driven (32x32 each).
fn icon_sizes() -> ImageSizes {
    let mut sizes = ImageSizes::new();
    sizes.insert("ui/icon_a".to_string(), [32.0, 32.0]);
    sizes.insert("ui/icon_b".to_string(), [32.0, 32.0]);
    sizes
}

/// Reproduce the renderer's gameplay decision: lay `tree` out (when present) and
/// return the draw data only if it is non-empty — `None` is the early-out signal
/// (no `begin_render_pass`). Mirrors `render_frame_indirect`'s
/// `if let Some(tree) ... if !draw.is_empty() { encode }`.
fn gameplay_draw(tree: Option<&AnchoredTree>, device_size: [u32; 2]) -> Option<UiDrawData> {
    let tree = tree?;
    let mut ui = UiTree::from_descriptor(tree);
    let mut fs = font_system();
    let draw = ui.build_draw_data(device_size, &mut fs, &icon_sizes());
    if draw.is_empty() { None } else { Some(draw) }
}

fn text(content: &str, font_size: f32) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size,
        color: [1.0, 1.0, 1.0, 1.0],
    })
}

/// A representative gameplay fixture exercising the full new vocab: an outer
/// vstack (flex column) with a backdrop `fill` (so it emits a panel quad sized to
/// its content), holding an hstack row (flex row) of two measured text leaves and
/// a 2-column grid of content-sized images.
fn composite_fixture() -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: 8.0,
            padding: 6.0,
            align: Align::Start,
            // Backdrop fill makes the outer container emit a panel quad sized to
            // its content (the canonical quad-producing path now that bare panels
            // have no intrinsic size).
            fill: Some([0.2, 0.3, 0.4, 1.0]),
            border: None,
            children: vec![
                Widget::HStack(ContainerWidget {
                    gap: 10.0,
                    padding: 0.0,
                    align: Align::Start,
                    fill: None,
                    border: None,
                    children: vec![text("HP 100", 24.0), text("ARMOR 50", 24.0)],
                }),
                Widget::Grid(GridWidget {
                    gap: 4.0,
                    padding: 0.0,
                    align: Align::Start,
                    cols: 2,
                    children: vec![
                        Widget::Image(ImageWidget {
                            asset: "ui/icon_a".into(),
                        }),
                        Widget::Image(ImageWidget {
                            asset: "ui/icon_b".into(),
                        }),
                    ],
                }),
            ],
        }),
    }
}

#[test]
fn gameplay_path_builds_non_empty_draw_list_from_descriptor_tree() {
    // The renderer builds a non-empty UI draw list from a descriptor tree on the
    // gameplay path. The composite fixture must produce panel quads, image quads
    // (grouped by asset key), and measured text runs — the full vocab.
    let fixture = composite_fixture();
    let draw = gameplay_draw(Some(&fixture), [1280, 720]).expect("non-empty tree opens the pass");

    assert!(!draw.quads.is_empty(), "the sized panel produced a quad");
    assert_eq!(
        draw.images.len(),
        2,
        "two image batches (one per asset key)"
    );
    assert_eq!(draw.images[0].0, "ui/icon_a");
    assert_eq!(draw.images[1].0, "ui/icon_b");
    assert_eq!(draw.texts.len(), 2, "two measured text runs");

    // Measured text sizing: the two runs differ in content, so real shaping gives
    // them different device widths (not a glyph-count estimate). Re-derive the
    // run order is stable, just assert both runs carry their content through.
    assert_eq!(draw.texts[0].content, "HP 100");
    assert_eq!(draw.texts[1].content, "ARMOR 50");

    // Integer pixel snap: every quad rect component is a whole device pixel.
    for q in draw
        .quads
        .instances
        .iter()
        .chain(draw.images.iter().flat_map(|(_, l)| l.instances.iter()))
    {
        for v in q.rect {
            assert!(
                (v - v.round()).abs() <= EPS,
                "gameplay quad rect component {v} not snapped to a whole device pixel",
            );
        }
    }
}

#[test]
fn snapshot_carries_gameplay_tree_as_the_content_contract() {
    // The widened `UiReadSnapshot` carries the descriptor tree (content side);
    // the renderer lays it out. A default snapshot carries no tree (the splash
    // path and any no-UI frame), which the gameplay path early-outs.
    let default = UiReadSnapshot::default();
    assert!(default.gameplay_tree.is_none(), "default carries no tree");

    let snapshot = UiReadSnapshot::with_gameplay_tree(composite_fixture());
    let tree = snapshot
        .gameplay_tree
        .as_ref()
        .expect("snapshot carries the gameplay tree");
    assert!(
        gameplay_draw(Some(tree), [1280, 720]).is_some(),
        "the snapshot's tree lays out to a non-empty draw list",
    );
}

#[test]
fn empty_gameplay_tree_early_outs_the_ui_pass() {
    // No tree in the snapshot -> early-out (no `begin_render_pass`).
    assert!(
        gameplay_draw(None, [1280, 720]).is_none(),
        "absent gameplay tree must early-out the UI pass",
    );

    // A structurally-present but drawable-empty tree (a container with no drawing
    // descendants) also early-outs: it lays out to zero quads / zero text.
    let empty = AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: 0.0,
            padding: 0.0,
            align: Align::Start,
            fill: None,
            border: None,
            children: vec![],
        }),
    };
    let draw_empty = {
        let mut ui = UiTree::from_descriptor(&empty);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &icon_sizes())
    };
    assert!(
        draw_empty.is_empty(),
        "an empty container tree lays out to no drawable output",
    );
    assert!(
        gameplay_draw(Some(&empty), [1280, 720]).is_none(),
        "an empty-laying-out tree must early-out the UI pass",
    );
}

#[test]
fn gameplay_tree_centers_against_letterbox_on_non_16_9() {
    // Anchor-against-letterbox on a non-16:9 viewport: a center-anchored fixture
    // shifts by the letterbox margin rather than stretching. At 1280x1440 the
    // canvas letterboxes vertically (scale 1.0, +360px y origin), so the same
    // fixture's draw moves down by 360 vs the 1280x720 result.
    let fixture = composite_fixture();
    let ref_draw = gameplay_draw(Some(&fixture), [1280, 720]).expect("non-empty");
    let letterboxed = gameplay_draw(Some(&fixture), [1280, 1440]).expect("non-empty");

    // The first text run's y shifts down by the vertical letterbox margin (360),
    // proving the whole tree anchors against the letterboxed canvas (scale stays
    // 1.0, so x is unchanged — uniform scale, not an x/y stretch).
    let dy = letterboxed.texts[0].position[1] - ref_draw.texts[0].position[1];
    assert!(
        (dy - 360.0).abs() <= 1.0,
        "letterboxed tree shifts down by the 360px margin, got {dy}",
    );
    let dx = letterboxed.texts[0].position[0] - ref_draw.texts[0].position[0];
    assert!(dx.abs() <= 1.0, "uniform scale: x unchanged, got dx {dx}");
}
