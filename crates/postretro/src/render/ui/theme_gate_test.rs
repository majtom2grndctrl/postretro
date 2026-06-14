// Hard-gate CPU assertions for M13 fonts+theming Task 4: the theme-generation
// rebuild gate and the fallback-and-warn "exactly one warning per tree build"
// contract.
//
// Two things this file pins that the per-module inline tests cannot:
//   - The retained gameplay rebuild gate is `descriptor != tree || generation
//     changed`. `UiPass::layout_gameplay_tree` owns that predicate but needs a
//     GPU `UiPass`, so this reproduces the SAME predicate CPU-side (the precedent
//     `gameplay_ui_gate_test` sets for reproducing a renderer decision headless),
//     and proves the rebuild produces new token values while a settled
//     descriptor+generation reuses the tree.
//   - Each unknown token logs EXACTLY ONE `log::warn!` per tree build (per
//     rebuild on the retained path, NOT per frame). A counting `log::Log` records
//     the warnings emitted while one `from_descriptor` runs.
//
// Pure CPU — no GPU adapter, no wgpu call.
//
// See: context/lib/ui.md

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

use super::descriptor::{
    Align, AnchoredTree, CaptureMode, ColorValue, ContainerWidget, SpacingValue, TextWidget, Widget,
};
use super::layout::Anchor;
use super::theme::{ThemeDescriptor, UiTheme};
use super::tree::UiTree;

fn font_system() -> glyphon::FontSystem {
    super::text::build_font_system()
}

fn no_images() -> super::tree::ImageSizes {
    super::tree::ImageSizes::new()
}

fn no_slots() -> HashMap<String, crate::scripting::slot_table::SlotValue> {
    HashMap::new()
}

// --- Theme-generation rebuild gate (CPU reproduction) ------------------------

/// Reproduce `UiPass::layout_gameplay_tree`'s rebuild predicate without a GPU
/// `UiPass`: a retained tree is reused while BOTH the descriptor and the theme
/// generation are unchanged; either differing forces a rebuild.
struct RetainedGate {
    descriptor: AnchoredTree,
    generation: u64,
    tree: UiTree,
    /// Number of times the gate actually rebuilt the `UiTree` (proxy for the
    /// `from_descriptor` rebuild the real gate performs).
    builds: u32,
}

impl RetainedGate {
    fn new(descriptor: &AnchoredTree, generation: u64, theme: &UiTheme) -> Self {
        Self {
            descriptor: descriptor.clone(),
            generation,
            tree: UiTree::from_descriptor(descriptor, theme),
            builds: 1,
        }
    }

    /// Run one frame: rebuild iff the descriptor or the generation changed, then
    /// lay out. Returns the resolved color of the first text run for assertions.
    fn frame(
        &mut self,
        descriptor: &AnchoredTree,
        generation: u64,
        theme: &UiTheme,
        fs: &mut glyphon::FontSystem,
    ) -> [u8; 4] {
        let needs_build = self.descriptor != *descriptor || self.generation != generation;
        if needs_build {
            self.descriptor = descriptor.clone();
            self.generation = generation;
            self.tree = UiTree::from_descriptor(descriptor, theme);
            self.builds += 1;
        }
        let data = self.tree.build_draw_data_retained(
            [1280, 720],
            fs,
            &no_images(),
            &no_slots(),
            &super::tree::CellValues::new(),
            0.0,
        );
        data.texts[0].color
    }
}

fn token_text(token: &str) -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color: ColorValue::Token(token.into()),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    }
}

fn srgb_of(linear: [f32; 4]) -> [u8; 4] {
    // Mirror tree::linear_rgba_to_srgb_u8 via a round-trip through a built tree:
    // build a literal-colored text and read its drawn color.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color: ColorValue::Literal(linear),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots())
        .texts[0]
        .color
}

#[test]
fn generation_bump_rebuilds_with_new_token_values_no_descriptor_change() {
    // Installing an override theme (modeled as a generation bump + new theme)
    // while a retained tree is alive rebuilds on the next frame with new token
    // values, even though the descriptor is byte-for-byte identical.
    let default = UiTheme::engine_default();
    let override_theme = default.with_override(&ThemeDescriptor {
        colors: HashMap::from([("critical".to_string(), [0.0, 1.0, 1.0, 1.0])]),
        ..Default::default()
    });
    let desc = token_text("critical");
    let mut fs = font_system();

    let mut gate = RetainedGate::new(&desc, 0, &default);
    let first = gate.frame(&desc, 0, &default, &mut fs);
    assert_eq!(gate.builds, 1, "first frame built once");
    assert_eq!(first, srgb_of(default.color("critical").unwrap()));

    // Generation bump (override installed): SAME descriptor, new generation.
    let second = gate.frame(&desc, 1, &override_theme, &mut fs);
    assert_eq!(
        gate.builds, 2,
        "a generation change rebuilds the retained tree (no descriptor change)",
    );
    assert_eq!(
        second,
        srgb_of([0.0, 1.0, 1.0, 1.0]),
        "the rebuilt tree carries the override theme's token value",
    );
}

#[test]
fn unchanged_generation_and_descriptor_does_not_rebuild() {
    // C's settled-frame guarantee must still hold: same descriptor + same
    // generation reuses the retained tree (no rebuild).
    let theme = UiTheme::engine_default();
    let desc = token_text("critical");
    let mut fs = font_system();

    let mut gate = RetainedGate::new(&desc, 3, &theme);
    gate.frame(&desc, 3, &theme, &mut fs);
    assert_eq!(gate.builds, 1, "first frame built once");

    gate.frame(&desc, 3, &theme, &mut fs);
    assert_eq!(
        gate.builds, 1,
        "an unchanged generation + descriptor must not rebuild",
    );
}

// --- Exactly-one-warning-per-build (counting logger) -------------------------

static WARN_COUNT: AtomicUsize = AtomicUsize::new(0);
static LOGGER_INIT: Once = Once::new();
/// Serializes the warning-count tests so the shared global `WARN_COUNT` is not
/// raced by parallel tests (the rest of the suite never inspects it).
static WARN_TEST_LOCK: Mutex<()> = Mutex::new(());

struct CountingLogger;

impl log::Log for CountingLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        // Count only the UI subsystem warnings the resolution sites emit.
        if record.level() == log::Level::Warn && record.args().to_string().contains("[UI]") {
            WARN_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn flush(&self) {}
}

fn install_logger() {
    LOGGER_INIT.call_once(|| {
        // Ignore an Err: another test (or env_logger) may have set a logger
        // first; the count is only meaningful under the serial lock anyway, and a
        // pre-installed logger means our counter never increments — guarded below.
        let _ = log::set_logger(&CountingLogger);
        log::set_max_level(log::LevelFilter::Warn);
    });
}

/// Build `desc` once against the engine default and return how many `[UI]`
/// warnings the single build emitted. Returns `None` if our counting logger is
/// not the active logger (some other test installed one first) so the caller can
/// skip rather than assert a meaningless zero.
fn warns_for_build(desc: &AnchoredTree) -> Option<usize> {
    let _guard = WARN_TEST_LOCK.lock().unwrap();
    install_logger();
    // If our logger isn't the global one, a probe warn won't register.
    WARN_COUNT.store(0, Ordering::SeqCst);
    log::warn!("[UI] logger-probe");
    if WARN_COUNT.load(Ordering::SeqCst) == 0 {
        return None;
    }
    WARN_COUNT.store(0, Ordering::SeqCst);
    let _ui = UiTree::from_descriptor(desc, &UiTheme::engine_default());
    Some(WARN_COUNT.load(Ordering::SeqCst))
}

#[test]
fn unknown_color_token_warns_exactly_once_per_build() {
    let desc = token_text("no.such.color");
    if let Some(n) = warns_for_build(&desc) {
        assert_eq!(n, 1, "one unknown color token logs exactly one warning");
    } else {
        eprintln!("[theme_gate_test] skipping: another logger is installed");
    }
}

#[test]
fn unknown_spacing_token_warns_exactly_once_per_build() {
    let desc = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: SpacingValue::Token("no.such.spacing".into()),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            children: vec![],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    };
    if let Some(n) = warns_for_build(&desc) {
        assert_eq!(n, 1, "one unknown spacing token logs exactly one warning");
    } else {
        eprintln!("[theme_gate_test] skipping: another logger is installed");
    }
}

#[test]
fn unknown_font_token_warns_exactly_once_per_build() {
    let desc = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0; 4]),
            font: Some("no.such.font".into()),
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    };
    if let Some(n) = warns_for_build(&desc) {
        assert_eq!(n, 1, "one unknown font token logs exactly one warning");
    } else {
        eprintln!("[theme_gate_test] skipping: another logger is installed");
    }
}
