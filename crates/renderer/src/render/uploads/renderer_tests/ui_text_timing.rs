// The UI stage's text prepare count, read through the real windowed record
// path (`record_ui_layer`) rather than `UiPass::encode` directly.
// See: context/lib/rendering_pipeline.md §12, context/lib/ui.md §5

use super::{record_window, renderer};
use crate::render::Renderer;
use crate::render::cpu_stages::RenderStage;
use crate::render::ui::text::TEXT_RECLAIM_CADENCE;
use postretro_stage_timing::TimingGate;
use postretro_ui::descriptor::{AnchoredTree, CaptureMode, ColorValue, TextWidget, Widget};
use postretro_ui::layout::Anchor;

fn text_layer(content: &str, offset: [f32; 2]) -> postretro_ui::UiTreeEntry {
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset,
        root: Widget::Text(TextWidget {
            content: content.into(),
            font_size: 24.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    postretro_ui::UiTreeEntry {
        name: "layer".into(),
        tier: postretro_ui::modal_stack::ScopeTier::Engine,
        capture_mode: tree.capture_mode,
        descriptor: tree,
        on_commit: None,
    }
}

/// Record and submit one windowed frame with `trees` as the modal stack, and
/// return the frame's prepare count as the stage set reports it.
fn frame_count(
    renderer: &mut Renderer,
    font: &mut postretro_ui::text::FontSystem,
    trees: &[postretro_ui::UiTreeEntry],
) -> Option<u64> {
    renderer.set_ui_snapshot(postretro_ui::UiReadSnapshot {
        trees: trees.to_vec(),
        ..Default::default()
    });
    // `render_frame_indirect` clears the frame record; the harness records
    // below it.
    renderer.cpu_frame.clear();
    let encoder = record_window(renderer, font);
    renderer.submit_windowed_frame(encoder);
    renderer
        .cpu_stages()
        .value(RenderStage::UiTextSpansPrepared)
}

#[test]
fn ui_frame_reports_text_spans_prepared_under_the_ui_stage() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_cpu_timing(TimingGate::ON);
    let mut font = postretro_ui::text::build_font_system();
    let hud = [
        text_layer("HP 100", [8.0, 8.0]),
        text_layer("AMMO 12", [8.0, 40.0]),
    ];

    assert_eq!(frame_count(&mut renderer, &mut font, &hud), Some(2));
    assert_eq!(
        frame_count(&mut renderer, &mut font, &hud),
        Some(0),
        "a settled frame reports the count present at zero"
    );
    assert_eq!(
        frame_count(&mut renderer, &mut font, &[]),
        Some(0),
        "a zero-text frame still reports the count"
    );
    // Folded as the binary folds the renderer's set, under its render stage:
    // the count is a present-at-zero sample nested under `rec_ui`.
    let mut record = postretro_stage_timing::FrameRecord::new();
    record.extend_from(renderer.cpu_stages(), Some("render"));
    let sample = record
        .samples()
        .iter()
        .find(|sample| sample.label == "ui_text_spans_prepared")
        .expect("the count is present on a zero-text UI frame");
    assert_eq!(sample.parent, Some("rec_ui"));
    assert_eq!(sample.kind, postretro_stage_timing::StageKind::Count);
    assert_eq!(sample.value, 0);

    // Back with text after the zero-text frame: both slots forgot their keys,
    // so the return frame is a reclaim, counting each live span once. Then a
    // settled window counts zero on every frame: no idle reclaim.
    let mut counts = Vec::new();
    for _ in 0..2 * TEXT_RECLAIM_CADENCE {
        counts.push(frame_count(&mut renderer, &mut font, &hud));
    }
    assert_eq!(counts[0], Some(2), "returning slots prepare again");
    assert!(
        counts[1..].iter().all(|&count| count == Some(0)),
        "settled frames prepared: {counts:?}"
    );
    eprintln!("[UiTextProof] UI text prepare count: 1 adapter case ran");
}

#[test]
fn ui_text_prepare_count_is_absent_with_timing_off() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    let hud = [text_layer("HP 100", [8.0, 8.0])];
    assert_eq!(frame_count(&mut renderer, &mut font, &hud), None);
    eprintln!("[UiTextProof] UI text prepare count off: 1 adapter case ran");
}
