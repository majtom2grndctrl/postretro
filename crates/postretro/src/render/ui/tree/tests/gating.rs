// Layout/draw dirty-gate and text-entry readout regression tests.

use super::common::*;
/// A two-leaf column tree, reused by the dirty-gating tests so they all lay
/// out the same shape.
fn gating_tree() -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: vstack(
            10.0,
            4.0,
            Align::Start,
            vec![text("AB", 30.0), text("CD", 30.0)],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

/// Regression: a bound `ui.textEntry` readout and an adjacent opener button
/// are DISTINCT retained nodes whose resolved drawn content can never alias
/// one another. Drives a focused fixture through the retained tree with a live
/// `ui.textEntry` value and asserts the readout draws `"ENTRY <value>"` while
/// the opener draws its immutable "ENTER TEXT" label — each at its own
/// position, with no per-node cache or auto-id collision swapping one node's
/// content/glyphs onto the other.
///
/// This pins the CPU half of the readout-aliasing bug (the reported symptom was
/// the readout rendering the opener's "ENTER TEXT" text): node identity is the
/// taffy `NodeId`, distinct per node, and `last_resolved` lives on the node, so
/// the readout's resolved string and the opener's literal label never cross.
/// (The GPU half — a single shared glyphon vertex buffer clobbered by a
/// per-stack-layer `encode` loop — is fixed in `render/mod.rs` and is not
/// CPU-testable without a GPU adapter; see that fix's note.)
#[test]
fn text_entry_readout_and_opener_resolve_distinct_non_aliasing_text() {
    let tree: AnchoredTree = serde_json::from_str(
        r#"{
                "anchor": "center",
                "offset": [0.0, 0.0],
                "root": {
                    "kind": "vstack",
                    "gap": 10.0,
                    "padding": 16.0,
                    "align": "stretch",
                    "children": [
                        {
                            "kind": "text",
                            "content": "ENTRY --",
                            "fontSize": 28.0,
                            "color": "ok",
                            "font": "mono",
                            "bind": {
                                "slot": "ui.textEntry",
                                "format": "ENTRY {}"
                            }
                        },
                        {
                            "kind": "button",
                            "id": "textEntryOpen",
                            "label": "ENTER TEXT",
                            "onPress": "openTextEntry"
                        }
                    ]
                },
                "captureMode": "capture"
            }"#,
    )
    .expect("focused text-entry fixture parses");
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let mut slots: HashMap<String, SlotValue> = HashMap::new();
    slots.insert(
        "ui.textEntry".to_string(),
        SlotValue::String("this is a test".to_string()),
    );
    let data =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);

    // The readout draws the bound value behind its untouched "ENTRY " prefix.
    let readout = data
        .texts
        .iter()
        .find(|t| t.content == "ENTRY this is a test")
        .expect("readout draws the bound ui.textEntry value behind the ENTRY prefix");
    // The opener button draws its immutable label, distinct from the readout.
    let opener = data
        .texts
        .iter()
        .find(|t| t.content == "ENTER TEXT")
        .expect("the opener button still draws its own ENTER TEXT label");

    // They are two separate draw entries at two separate positions — neither
    // node picked up the other's resolved content (the aliasing symptom).
    assert_ne!(
        readout.position, opener.position,
        "the readout and opener are distinct nodes at distinct positions",
    );
    // No drawn run is the opener's label masquerading as the readout: exactly
    // one run carries each string.
    assert_eq!(
        data.texts
            .iter()
            .filter(|t| t.content == "ENTER TEXT")
            .count(),
        1,
        "the ENTER TEXT label appears exactly once (only on the opener node)",
    );
    assert_eq!(
        data.texts
            .iter()
            .filter(|t| t.content == "ENTRY this is a test")
            .count(),
        1,
        "the resolved readout string appears exactly once (only on the readout node)",
    );

    // The readout node's `last_resolved` holds ITS value; the opener node is
    // unbound and never resolves — so the two per-node caches cannot cross.
    let mut ids = Vec::new();
    ui.collect_node_ids(ui.root, &mut ids);
    let mut readout_resolved = None;
    let mut saw_unbound_opener = false;
    for n in ids {
        if let Some(NodeContext::Text {
            content,
            last_resolved,
            bind,
            ..
        }) = ui.taffy.get_node_context(n)
        {
            if bind.as_ref().and_then(|b| b.source.slot()) == Some("ui.textEntry") {
                readout_resolved = last_resolved.clone();
            }
            if content == "ENTER TEXT" {
                saw_unbound_opener = true;
                assert!(bind.is_none(), "the opener label is unbound");
                assert!(
                    last_resolved.is_none(),
                    "the unbound opener never resolves a bound string",
                );
            }
        }
    }
    assert_eq!(
        readout_resolved.as_deref(),
        Some("ENTRY this is a test"),
        "the readout node caches its OWN resolved string, not the opener's label",
    );
    assert!(saw_unbound_opener, "the opener node exists in the tree");
}

#[test]
fn unchanged_frame_reuses_cached_layout_without_recompute() {
    // First layout populates taffy's cache (count 1); a second call with the
    // same tree and same viewport hits the gate's no-change path and reuses
    // the cached subtree layout — the compute counter must stay flat.
    let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
    let mut fs = font_system();

    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(ui.recompute_count(), 1, "first layout computes once");

    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        ui.recompute_count(),
        1,
        "same tree + same viewport must not recompute",
    );
}

#[test]
fn viewport_change_forces_layout_recompute() {
    // A different device size re-resolves the letterbox/scale, so the gate
    // must recompute even though the tree is byte-for-byte identical.
    let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
    let mut fs = font_system();

    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(ui.recompute_count(), 1);

    ui.build_draw_data([3840, 2160], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        ui.recompute_count(),
        2,
        "a changed viewport must trigger a recompute",
    );
}

#[test]
fn rebuilt_tree_recomputes_from_empty_cache() {
    // Structural change = a new tree built from a (possibly new) descriptor.
    // The fresh tree's root cache is empty, so its first layout computes even
    // at the same viewport the previous tree was laid out against.
    let mut fs = font_system();

    let mut first = UiTree::from_descriptor(&gating_tree(), &theme());
    first.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    first.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(first.recompute_count(), 1, "cached after the first layout");

    // Reshape: a structurally different descriptor yields a new tree, which
    // must recompute on its first layout regardless of viewport.
    let reshaped = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: vstack(
            10.0,
            4.0,
            Align::Start,
            vec![text("AB", 30.0), text("CD", 30.0), text("EF", 30.0)],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut second = UiTree::from_descriptor(&reshaped, &theme());
    second.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        second.recompute_count(),
        1,
        "a rebuilt/reshaped tree recomputes on its first layout",
    );
}

#[test]
fn cached_frame_draw_data_matches_recomputed_frame() {
    // The gate skips the *compute*, not the draw-list production. The cached
    // frame reads back the same taffy::Layout rects, so its draw data must be
    // identical to the freshly-computed frame's.
    let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
    let mut fs = font_system();

    let computed = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let cached = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    // Confirm the second call really took the cached path.
    assert_eq!(ui.recompute_count(), 1, "second frame did not recompute");

    assert_eq!(computed.quads.instances.len(), cached.quads.instances.len());
    assert_eq!(computed.texts.len(), cached.texts.len());
    for (a, b) in computed.texts.iter().zip(cached.texts.iter()) {
        assert!(
            approx(a.position[0], b.position[0]) && approx(a.position[1], b.position[1]),
            "cached text position {:?} differs from computed {:?}",
            b.position,
            a.position,
        );
        assert!(
            approx(a.font_size, b.font_size),
            "cached font size {} differs from computed {}",
            b.font_size,
            a.font_size,
        );
        assert_eq!(a.content, b.content, "cached text content differs");
    }
}
