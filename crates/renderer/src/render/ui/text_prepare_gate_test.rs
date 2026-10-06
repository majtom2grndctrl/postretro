// Headless multi-frame goldens for the change-gated text prepare: which spans
// prepare on which frame, when the atlas is trimmed, and that every frame's
// pixels equal a fresh pass's render of the same composition — the oracle for
// "text never goes stale or missing". Self-skips when no GPU adapter is present.
//
// Positions and font sizes here are device pixels, the space `UiText` arrives
// in after UI scaling, so the glyph sizes the atlas sees are the ones named.
//
// See: context/lib/ui.md §5, context/plans/in-progress/ui-text-prepare-churn

use super::text::TEXT_RECLAIM_CADENCE;
use super::tree::UiDrawData;
use super::{TextPrepareStats, UiComposition, UiInstance, UiPass, UiText};
use crate::render::gpu_test_harness::{
    GpuCtx, Readback, read_texture_rgba8_staged, try_init_gpu, try_init_gpu_with_limits,
};
use crate::render::uploads::UploadQueue;

const W: u32 = 256;
const H: u32 = 192;
const SIZE: f32 = 24.0;
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn text(content: &str, position: [f32; 2], font_size: f32) -> UiText {
    UiText::new(
        content,
        position,
        font_size,
        WHITE,
        postretro_ui::text::UI_FONT_FAMILY,
    )
}

/// One layer's paint stream, in order.
enum Item {
    Text(UiText),
    Panel([f32; 4], [f32; 4]),
}

fn layer(items: Vec<Item>) -> UiDrawData {
    let mut draw = UiDrawData::default();
    for item in items {
        match item {
            Item::Text(t) => draw.push_text(t),
            Item::Panel(rect, color) => draw.push_quad(UiInstance::panel(rect, color, [0.0; 4])),
        }
    }
    draw
}

fn t(content: &str, x: f32, y: f32) -> Item {
    Item::Text(text(content, [x, y], SIZE))
}

/// One pass driven frame after frame, as the windowed path drives it: encode,
/// submit, `mark_submitted`.
struct Rig {
    ctx: GpuCtx,
    pass: UiPass,
    font_system: postretro_ui::text::FontSystem,
    size: [u32; 2],
}

struct Frame {
    stats: TextPrepareStats,
    prepared: Vec<(usize, usize)>,
    pixels: Readback,
}

impl Rig {
    fn new(ctx: GpuCtx) -> Self {
        let pass = UiPass::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        Self {
            ctx,
            pass,
            font_system: postretro_ui::text::build_font_system(),
            size: [W, H],
        }
    }

    fn frame(&mut self, layers: &[UiDrawData]) -> Frame {
        let (stats, pixels) = encode_and_read(
            &self.ctx,
            &mut self.pass,
            &mut self.font_system,
            self.size,
            layers,
        );
        self.pass.mark_submitted();
        Frame {
            stats,
            prepared: self.pass.text_prepared_spans_for_test(),
            pixels,
        }
    }

    /// The same composition through a fresh pass: no retained span, no
    /// retained atlas. Equal pixels mean nothing the gate skipped went stale.
    fn reference(&self, layers: &[UiDrawData]) -> Readback {
        let mut pass = UiPass::new(
            &self.ctx.device,
            &self.ctx.queue,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        let mut font_system = postretro_ui::text::build_font_system();
        encode_and_read(&self.ctx, &mut pass, &mut font_system, self.size, layers).1
    }

    fn assert_matches_reference(&self, frame: &Frame, layers: &[UiDrawData], what: &str) {
        let reference = self.reference(layers);
        let differing = differing_pixels(&frame.pixels, &reference);
        assert_eq!(
            differing, 0,
            "{what}: {differing} pixels differ from a fresh pass's render of the same composition",
        );
    }
}

fn encode_and_read(
    ctx: &GpuCtx,
    pass: &mut UiPass,
    font_system: &mut postretro_ui::text::FontSystem,
    size: [u32; 2],
    layers: &[UiDrawData],
) -> (TextPrepareStats, Readback) {
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("text_prepare_gate target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let white = pass.white_bind_group().clone();
    let images = super::UiImageRegistry::default();
    let composition = UiComposition::from_layer_draws(layers, &white, &images);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("text_prepare_gate encoder"),
        });
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let stats = pass.encode(
        font_system,
        &ctx.device,
        &uploads,
        &mut encoder,
        &view,
        size,
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        &composition,
    );
    let pixels = read_texture_rgba8_staged(ctx, &uploads, &target, size[0], size[1], encoder);
    (stats, pixels)
}

fn differing_pixels(a: &Readback, b: &Readback) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .filter(|(x, y)| x != y)
        .count()
}

/// The pixels of rows `y0..y1`, for comparing one span's band across frames.
fn band(rb: &Readback, y0: u32, y1: u32) -> Vec<u8> {
    let stride = (rb.width * 4) as usize;
    rb.pixels[y0 as usize * stride..y1.min(rb.height) as usize * stride].to_vec()
}

fn ink(rb: &Readback, y0: u32, y1: u32) -> usize {
    band(rb, y0, y1)
        .chunks_exact(4)
        .filter(|p| p[0] > 48 || p[1] > 48 || p[2] > 48)
        .count()
}

macro_rules! gpu_or_skip {
    ($init:expr) => {
        match $init {
            Some(ctx) => ctx,
            None => {
                eprintln!("[text_prepare_gate_test] skipping: no GPU adapter available");
                return;
            }
        }
    };
}

#[test]
fn identical_frames_prepare_nothing_and_match_pixels() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let layers = [layer(vec![
        t("HP 100", 8.0, 8.0),
        Item::Panel([8.0, 60.0, 80.0, 8.0], [0.2, 0.8, 0.2, 1.0]),
        t("AMMO 12", 8.0, 90.0),
    ])];

    let first = rig.frame(&layers);
    assert_eq!(first.prepared, vec![(0, 0), (0, 1)]);
    rig.assert_matches_reference(&first, &layers, "first frame");

    let second = rig.frame(&layers);
    assert_eq!(
        second.stats,
        TextPrepareStats::default(),
        "an unchanged frame prepares, writes and trims nothing"
    );
    assert_eq!(differing_pixels(&first.pixels, &second.pixels), 0);
}

#[test]
fn one_changed_span_prepares_only_itself_then_nothing() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let frame_layers = |ammo: &str| {
        [
            layer(vec![t("HP 100", 8.0, 8.0)]),
            layer(vec![t(ammo, 8.0, 90.0)]),
        ]
    };

    let before = frame_layers("AMMO 12");
    let first = rig.frame(&before);
    assert_eq!(first.prepared, vec![(0, 0), (1, 0)]);

    let after = frame_layers("AMMO 11");
    let changed = rig.frame(&after);
    assert_eq!(
        changed.prepared,
        vec![(1, 0)],
        "only the changed span prepares"
    );
    assert_eq!(changed.stats.spans_prepared, 1);
    assert!(!changed.stats.trimmed);
    rig.assert_matches_reference(&changed, &after, "changed frame");
    assert_eq!(
        band(&changed.pixels, 0, 60),
        band(&first.pixels, 0, 60),
        "the unchanged span's pixels hold",
    );

    let settled = rig.frame(&after);
    assert_eq!(settled.stats, TextPrepareStats::default());
}

#[test]
fn changing_span_reclaims_on_cadence_and_static_pixels_hold() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let frames = 2 * TEXT_RECLAIM_CADENCE + 7;
    let mut static_band = None;
    let mut since_reclaim = 0;
    let mut reclaims = 0;
    for i in 0..frames {
        let layers = [
            layer(vec![t("STATIC LABEL", 8.0, 8.0)]),
            layer(vec![t(&format!("COUNT {i}"), 8.0, 90.0)]),
        ];
        let frame = rig.frame(&layers);
        since_reclaim += 1;
        if i == 0 {
            assert_eq!(frame.stats.spans_prepared, 2);
        } else if frame.stats.reclaimed {
            assert!(frame.stats.trimmed);
            assert_eq!(
                frame.prepared,
                vec![(0, 0), (1, 0)],
                "a reclaim prepares every live span"
            );
            reclaims += 1;
            since_reclaim = 0;
        } else {
            assert!(!frame.stats.trimmed, "frame {i} trimmed outside a reclaim");
            assert_eq!(frame.prepared, vec![(1, 0)], "frame {i}");
        }
        assert!(
            since_reclaim <= TEXT_RECLAIM_CADENCE,
            "no reclaim within one cadence by frame {i}"
        );
        let band = band(&frame.pixels, 0, 60);
        match &static_band {
            None => static_band = Some(band),
            Some(first) => assert!(*first == band, "static span changed on frame {i}"),
        }
        if frame.stats.reclaimed || i + 1 == frames {
            rig.assert_matches_reference(&frame, &layers, &format!("frame {i}"));
        }
    }
    assert!(reclaims >= 2, "only {reclaims} reclaims in {frames} frames");
}

/// Static label beside a counter that needs a new glyph every frame, with the
/// atlas held at its initial 256 px by the device limit so it cannot grow. The
/// in-use set fills within a few dozen frames, so the run crosses atlas-full
/// recoveries and reclaims. A skipped span's glyphs must survive every
/// eviction in between.
#[test]
fn pinned_atlas_new_glyph_counter_keeps_static_label() {
    const LIMIT: u32 = 256;
    let limits = wgpu::Limits {
        max_texture_dimension_2d: LIMIT,
        ..wgpu::Limits::default()
    };
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu_with_limits(limits)));
    rig.size = [LIMIT, LIMIT];
    assert_eq!(rig.ctx.device.limits().max_texture_dimension_2d, LIMIT);

    const GLYPHS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let glyphs: Vec<char> = GLYPHS.chars().collect();
    let frames = 2 * TEXT_RECLAIM_CADENCE + 7;
    let mut static_band = None;
    let mut reclaims = 0;
    let mut atlas_full = 0;
    let mut trimmed_last_frame = false;
    for i in 0..frames as usize {
        let counter = glyphs[i % glyphs.len()].to_string();
        let layers = [
            layer(vec![t("STATIC", 8.0, 8.0)]),
            layer(vec![Item::Text(text(&counter, [8.0, 80.0], 72.0))]),
        ];
        let frame = rig.frame(&layers);
        if frame.stats.atlas_full_recovered {
            atlas_full += 1;
            assert_eq!(
                frame.prepared,
                vec![(0, 0), (1, 0)],
                "an atlas-full frame prepares every live span again"
            );
            assert_eq!(
                frame.stats.spans_prepared, 2,
                "an atlas-full frame counts each live span once"
            );
        }
        if frame.stats.trimmed {
            reclaims += 1;
        }
        let band = band(&frame.pixels, 0, 60);
        match &static_band {
            None => static_band = Some(band),
            Some(first) => assert!(*first == band, "static span changed on frame {i}"),
        }
        // The static band is checked every frame; the whole frame against a
        // fresh pass around every trim and periodically between them.
        if frame.stats.trimmed || trimmed_last_frame || i % 8 == 0 {
            rig.assert_matches_reference(&frame, &layers, &format!("frame {i}"));
        }
        trimmed_last_frame = frame.stats.trimmed;
    }
    assert!(atlas_full >= 1, "the run never filled the atlas");
    assert!(reclaims >= 2, "only {reclaims} reclaims in {frames} frames");
}

fn layers_of(prepared: &[(usize, usize)]) -> Vec<usize> {
    let mut layers: Vec<usize> = prepared.iter().map(|&(layer, _)| layer).collect();
    layers.dedup();
    layers
}

fn pinned_atlas_rig() -> Option<Rig> {
    const LIMIT: u32 = 256;
    let limits = wgpu::Limits {
        max_texture_dimension_2d: LIMIT,
        ..wgpu::Limits::default()
    };
    let mut rig = Rig::new(try_init_gpu_with_limits(limits)?);
    // Target and depth target must fit the same limit as the atlas.
    rig.size = [LIMIT, LIMIT];
    Some(rig)
}

#[test]
fn zero_text_frame_prepares_nothing_and_defers_reclaim() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let with_text = [layer(vec![t("HP 100", 8.0, 8.0)])];
    let without_text = [UiDrawData::default()];

    // Bring the reclaim to the brink: the next encode with text is due.
    for _ in 0..TEXT_RECLAIM_CADENCE - 1 {
        let frame = rig.frame(&with_text);
        assert!(!frame.stats.reclaimed);
    }

    let empty = rig.frame(&without_text);
    assert_eq!(
        empty.stats,
        TextPrepareStats::default(),
        "a zero-text frame prepares, writes and trims nothing"
    );
    assert_eq!(ink(&empty.pixels, 0, H), 0, "the layer clears with no text");

    let back = rig.frame(&with_text);
    assert!(
        back.stats.reclaimed,
        "the reclaim that fell due moves to the next frame with text"
    );
    assert_eq!(back.prepared, vec![(0, 0)]);
    rig.assert_matches_reference(&back, &with_text, "frame after zero text");
}

#[test]
fn viewport_change_reprepares_every_span_once() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let layers = [
        layer(vec![t("HP 100", 8.0, 8.0)]),
        layer(vec![t("AMMO 12", 8.0, 90.0)]),
    ];
    rig.frame(&layers);

    rig.size = [320, 200];
    let resized = rig.frame(&layers);
    assert_eq!(resized.prepared, vec![(0, 0), (1, 0)]);
    rig.assert_matches_reference(&resized, &layers, "resized frame");

    let settled = rig.frame(&layers);
    assert_eq!(settled.stats, TextPrepareStats::default());
}

#[test]
fn appearance_change_prepares_only_its_span() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |size: f32, color: [u8; 4]| {
        let mut ammo = text("AMMO 12", [8.0, 90.0], size);
        ammo.color = color;
        [
            layer(vec![t("HP 100", 8.0, 8.0)]),
            layer(vec![Item::Text(ammo)]),
        ]
    };
    rig.frame(&build(SIZE, WHITE));

    for (what, layers) in [
        ("font size", build(SIZE * 1.5, WHITE)),
        ("colour", build(SIZE * 1.5, [255, 64, 64, 255])),
    ] {
        let changed = rig.frame(&layers);
        assert_eq!(changed.prepared, vec![(1, 0)], "{what} change");
        rig.assert_matches_reference(&changed, &layers, what);
        let settled = rig.frame(&layers);
        assert_eq!(settled.stats, TextPrepareStats::default(), "after {what}");
    }
}

#[test]
fn shape_insert_prepares_only_its_layer() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |panel: bool| {
        let mut hud = vec![t("AMMO 12", 8.0, 60.0)];
        if panel {
            hud.push(Item::Panel([8.0, 90.0, 60.0, 6.0], [0.2, 0.8, 0.2, 1.0]));
        }
        hud.push(t("RESERVE 90", 8.0, 110.0));
        [layer(vec![t("HP 100", 8.0, 8.0)]), layer(hud)]
    };
    rig.frame(&build(false));

    let inserted = rig.frame(&build(true));
    assert!(!inserted.prepared.is_empty());
    assert_eq!(
        layers_of(&inserted.prepared),
        vec![1],
        "only the shape's layer"
    );
    rig.assert_matches_reference(&inserted, &build(true), "shape inserted");
    assert_eq!(rig.frame(&build(true)).stats, TextPrepareStats::default());

    let removed = rig.frame(&build(false));
    assert_eq!(layers_of(&removed.prepared), vec![1]);
    assert_eq!(rig.frame(&build(false)).stats, TextPrepareStats::default());
}

#[test]
fn font_registration_prepares_every_span_once() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let layers = [
        layer(vec![t("HP 100", 8.0, 8.0)]),
        layer(vec![t("AMMO 12", 8.0, 90.0)]),
    ];
    rig.frame(&layers);

    // A rejected registration still moves the generation: `false` can follow
    // faces added under another family name.
    let registered =
        rig.pass
            .register_font(&mut rig.font_system, "NotAFace", b"not a font".to_vec());
    assert!(!registered);
    let after = rig.frame(&layers);
    assert_eq!(after.prepared, vec![(0, 0), (1, 0)]);
    assert_eq!(rig.frame(&layers).stats, TextPrepareStats::default());
}

/// The presentation layer folds first (layer 0), so a damage number spawning
/// or despawning there must not reslot or re-depth any HUD or modal span.
#[test]
fn presentation_spawn_prepares_no_other_layer() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |numbers: &[&str]| {
        // Each number is its own template instance with a status bar, so the
        // presentation layer holds one span per instance.
        let presentation = layer(
            numbers
                .iter()
                .enumerate()
                .flat_map(|(i, n)| {
                    let y = 20.0 + 30.0 * i as f32;
                    [
                        t(n, 120.0, y),
                        Item::Panel([120.0, y + 24.0, 30.0, 3.0], [0.9, 0.1, 0.1, 1.0]),
                    ]
                })
                .collect(),
        );
        [
            presentation,
            layer(vec![t("HP 100", 8.0, 8.0)]),
            layer(vec![t("AMMO 12", 8.0, 150.0)]),
        ]
    };
    rig.frame(&build(&["12"]));

    for (what, numbers) in [("spawn", &["12", "34"][..]), ("despawn", &["34"][..])] {
        let frame = rig.frame(&build(numbers));
        assert!(
            frame.prepared.iter().all(|&(layer, _)| layer == 0),
            "{what} prepared outside the presentation layer: {:?}",
            frame.prepared
        );
        rig.assert_matches_reference(&frame, &build(numbers), what);
        let next = rig.frame(&build(numbers));
        assert_eq!(
            next.stats,
            TextPrepareStats::default(),
            "frame after {what}"
        );
    }
}

#[test]
fn middle_layer_meter_toggle_prepares_no_other_layer() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |meter: bool| {
        let mut ammo = vec![t("AMMO 12", 8.0, 60.0)];
        if meter {
            ammo.insert(0, Item::Panel([8.0, 50.0, 80.0, 6.0], [0.9, 0.9, 0.1, 1.0]));
        }
        [
            UiDrawData::default(),
            layer(vec![t("HP 100", 8.0, 8.0)]),
            layer(ammo),
            layer(vec![t("XP 7", 8.0, 150.0)]),
        ]
    };
    rig.frame(&build(false));

    for (what, meter) in [("show", true), ("hide", false)] {
        let frame = rig.frame(&build(meter));
        assert!(
            frame.prepared.iter().all(|&(layer, _)| layer == 2),
            "meter {what} prepared outside its layer: {:?}",
            frame.prepared
        );
        rig.assert_matches_reference(&frame, &build(meter), what);
        assert_eq!(
            rig.frame(&build(meter)).stats,
            TextPrepareStats::default(),
            "frame after meter {what}"
        );
    }
}

/// `hud.ammo` then `hud.openSeats`: text on both sides of a layer boundary.
#[test]
fn layer_boundary_ends_span() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |reserve: &str| {
        [
            layer(vec![t("AMMO 12", 8.0, 8.0), t(reserve, 8.0, 40.0)]),
            layer(vec![t("SEATS 3", 8.0, 120.0)]),
        ]
    };
    let layers = build("RESERVE 90");
    let white = rig.pass.white_bind_group().clone();
    let images = super::UiImageRegistry::default();
    let composition = UiComposition::from_layer_draws(&layers, &white, &images);
    let spans: Vec<(usize, usize, std::ops::Range<usize>)> = composition
        .text_batches
        .iter()
        .map(|b| (b.layer, b.span, b.range.clone()))
        .collect();
    assert_eq!(spans, vec![(0, 0, 0..2), (1, 0, 2..3)]);
    drop(composition);

    rig.frame(&layers);
    let changed = rig.frame(&build("RESERVE 89"));
    assert_eq!(changed.prepared, vec![(0, 0)], "the upper layer holds");
    rig.assert_matches_reference(&changed, &build("RESERVE 89"), "lower change");
}

#[test]
fn modal_push_prepares_only_pushed_layer_and_pop_prepares_nothing() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let hud = [UiDrawData::default(), layer(vec![t("HP 100", 8.0, 8.0)])];
    let menu = layer(vec![
        Item::Panel([40.0, 40.0, 176.0, 112.0], [0.1, 0.1, 0.3, 1.0]),
        t("PAUSED", 60.0, 50.0),
        t("RESUME", 60.0, 90.0),
    ]);
    let paused = [hud[0].clone(), hud[1].clone(), menu];
    rig.frame(&hud);

    let opened = rig.frame(&paused);
    assert!(!opened.prepared.is_empty());
    assert_eq!(
        layers_of(&opened.prepared),
        vec![2],
        "only the menu prepares"
    );
    rig.assert_matches_reference(&opened, &paused, "menu open");

    let closed = rig.frame(&hud);
    assert_eq!(closed.stats, TextPrepareStats::default());
    rig.assert_matches_reference(&closed, &hud, "menu closed");
}

#[test]
fn banded_depth_keeps_own_panel_under_own_text() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let layers = [
        layer(vec![
            Item::Panel([0.0, 0.0, W as f32, 60.0], [0.0, 0.0, 1.0, 1.0]),
            t("OVER OWN PANEL", 8.0, 8.0),
            t("UNDER UPPER PANEL", 8.0, 100.0),
        ]),
        layer(vec![
            Item::Panel([0.0, 90.0, W as f32, 50.0], [1.0, 0.0, 0.0, 1.0]),
            t("UPPER", 8.0, 150.0),
        ]),
    ];
    let frame = rig.frame(&layers);
    // White text over a blue panel is the only red in the top band.
    let text_over_panel = band(&frame.pixels, 0, 60)
        .chunks_exact(4)
        .filter(|p| p[0] > 128)
        .count();
    assert!(
        text_over_panel > 0,
        "the layer's text drew over its own panel"
    );
    let leaked = band(&frame.pixels, 95, 135)
        .chunks_exact(4)
        .filter(|p| p[1] > 48 || p[2] > 48)
        .count();
    assert!(
        leaked < 8,
        "lower text leaked over the upper panel: {leaked} px"
    );
    assert!(ink(&frame.pixels, 150, H) > 0, "upper text drew");
}

#[test]
fn returning_layer_reprepares_and_draws_correct_text() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let hud = [layer(vec![t("HP 100", 8.0, 8.0)])];
    let with_menu = [hud[0].clone(), layer(vec![t("PAUSED", 60.0, 90.0)])];

    // Away across a viewport change, then back at the stored size (O6).
    rig.frame(&with_menu);
    rig.frame(&hud);
    rig.size = [320, 200];
    rig.frame(&hud);
    rig.size = [W, H];
    rig.frame(&hud);
    let back = rig.frame(&with_menu);
    assert!(
        back.prepared.contains(&(1, 0)),
        "a returning slot prepares again with identical text"
    );
    rig.assert_matches_reference(&back, &with_menu, "back after resize");

    // Away across a cadence reclaim.
    rig.frame(&hud);
    let mut reclaimed = false;
    for _ in 0..TEXT_RECLAIM_CADENCE {
        reclaimed |= rig.frame(&hud).stats.reclaimed;
    }
    assert!(reclaimed);
    let back = rig.frame(&with_menu);
    assert!(back.prepared.contains(&(1, 0)));
    rig.assert_matches_reference(&back, &with_menu, "back after reclaim");

    // A span count that shrinks: the slot beyond it draws nothing.
    let two_spans = [layer(vec![
        t("HP 100", 8.0, 8.0),
        Item::Panel([8.0, 40.0, 40.0, 4.0], [0.2, 0.8, 0.2, 1.0]),
        t("SHIELD 50", 8.0, 60.0),
    ])];
    rig.frame(&two_spans);
    let shrunk = rig.frame(&hud);
    rig.assert_matches_reference(&shrunk, &hud, "span count shrank");
}

#[test]
fn returning_layer_is_correct_after_atlas_full_while_away() {
    let Some(mut rig) = pinned_atlas_rig() else {
        eprintln!("[text_prepare_gate_test] skipping: no GPU adapter available");
        return;
    };
    let menu = layer(vec![Item::Text(text("PAUSED", [8.0, 8.0], 48.0))]);
    let counter = |c: char| layer(vec![Item::Text(text(&c.to_string(), [8.0, 120.0], 72.0))]);

    rig.frame(&[counter('A'), menu.clone()]);
    let mut full = false;
    for c in "BCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".chars() {
        full |= rig.frame(&[counter(c)]).stats.atlas_full_recovered;
        if full {
            break;
        }
    }
    assert!(full, "the away period never filled the atlas");
    let layers = [counter('Z'), menu];
    let back = rig.frame(&layers);
    assert!(back.prepared.contains(&(1, 0)));
    rig.assert_matches_reference(&back, &layers, "back after atlas-full");
}

/// The lower layer gains a span, then the atlas is trimmed: unchanged spans
/// re-prepare from retained buffers and must keep their own depths, because a
/// glyph's depth index is slot-local (O22).
fn assert_trims_after_lower_change_keep_occlusion(rig: &mut Rig, counter_size: f32) {
    let build = |lower_spans: usize, i: usize| {
        let mut lower = vec![t("LOWER A", 8.0, 8.0)];
        for s in 1..lower_spans {
            let y = 30.0 * s as f32;
            lower.push(Item::Panel([8.0, y, 20.0, 2.0], [0.2, 0.8, 0.2, 1.0]));
            lower.push(t("LOWER B", 120.0, y));
        }
        lower.push(t("HIDDEN", 8.0, 100.0));
        let glyph = char::from(b'A' + (i % 26) as u8).to_string();
        [
            layer(lower),
            layer(vec![
                Item::Panel([0.0, 90.0, 110.0, 40.0], [1.0, 0.0, 0.0, 1.0]),
                t("ON PANEL", 8.0, 95.0),
            ]),
            layer(vec![Item::Text(text(&glyph, [180.0, 120.0], counter_size))]),
        ]
    };
    rig.frame(&build(1, 0));
    let mut trims = 0;
    for i in 1..=(TEXT_RECLAIM_CADENCE as usize + 2) {
        let layers = build(2, i);
        let frame = rig.frame(&layers);
        if frame.stats.trimmed {
            trims += 1;
            rig.assert_matches_reference(&frame, &layers, &format!("trim frame {i}"));
        }
    }
    assert!(trims >= 1, "the run never trimmed");
}

#[test]
fn reclaim_after_lower_layer_change_keeps_occlusion() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    assert_trims_after_lower_change_keep_occlusion(&mut rig, SIZE);
}

#[test]
fn atlas_full_after_lower_layer_change_keeps_occlusion() {
    let Some(mut rig) = pinned_atlas_rig() else {
        eprintln!("[text_prepare_gate_test] skipping: no GPU adapter available");
        return;
    };
    assert_trims_after_lower_change_keep_occlusion(&mut rig, 96.0);
}

#[test]
fn over_band_bound_falls_back_then_reprepares_once() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let build = |layer_count: usize| {
        let mut layers: Vec<UiDrawData> = (0..layer_count - 1)
            .map(|i| {
                let x = 8.0 + 30.0 * (i % 8) as f32;
                let y = 8.0 + 40.0 * (i / 8) as f32;
                layer(vec![t(&format!("{i}"), x, y)])
            })
            .collect();
        // Top layer: an opaque panel over the first row of lower text.
        layers.push(layer(vec![
            Item::Panel([0.0, 0.0, W as f32, 40.0], [1.0, 0.0, 0.0, 1.0]),
            t("TOP", 8.0, 170.0),
        ]));
        layers
    };
    let bound = super::composition::UI_DEPTH_BANDS;

    let within = build(bound);
    rig.frame(&within);
    let over = build(bound + 1);
    let fallback = rig.frame(&over);
    rig.assert_matches_reference(&fallback, &over, "over the band bound");
    let leaked = band(&fallback.pixels, 0, 40)
        .chunks_exact(4)
        .filter(|p| p[1] > 48 || p[2] > 48)
        .count();
    assert!(
        leaked < 8,
        "lower text leaked over the top panel: {leaked} px"
    );

    let back = rig.frame(&within);
    assert_eq!(
        back.stats.spans_prepared as usize, bound,
        "back within the bound, every span prepares once"
    );
    assert_eq!(rig.frame(&within).stats, TextPrepareStats::default());
}

/// A glyph larger than the pinned atlas can never fit, so its span's prepare
/// fails even after the atlas-full recovery (O4).
#[test]
fn failed_prepare_draws_nothing_and_retries() {
    let Some(mut rig) = pinned_atlas_rig() else {
        eprintln!("[text_prepare_gate_test] skipping: no GPU adapter available");
        return;
    };
    let ok = layer(vec![t("OK", 8.0, 8.0)]);
    let layers = [
        ok.clone(),
        layer(vec![Item::Text(text("W", [0.0, 0.0], 400.0))]),
    ];

    for frame_index in 0..2 {
        let frame = rig.frame(&layers);
        assert!(frame.stats.atlas_full_recovered);
        assert!(
            frame.prepared.contains(&(1, 0)),
            "frame {frame_index}: the failed span prepares again though unchanged"
        );
        let only_ok = rig.reference(std::slice::from_ref(&ok));
        assert_eq!(
            differing_pixels(&frame.pixels, &only_ok),
            0,
            "frame {frame_index}: the failed span drew something"
        );
    }
}

/// O20: the guard counts encodes, so it trips even when the first encode
/// prepared nothing.
#[cfg(debug_assertions)]
#[test]
fn second_encode_before_submit_trips_guard_even_without_prepare() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    let empty = [UiDrawData::default()];
    let (stats, _) = encode_and_read(
        &rig.ctx,
        &mut rig.pass,
        &mut rig.font_system,
        rig.size,
        &empty,
    );
    assert_eq!(stats.spans_prepared, 0);
    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        encode_and_read(
            &rig.ctx,
            &mut rig.pass,
            &mut rig.font_system,
            rig.size,
            &empty,
        )
    }));
    assert!(
        second.is_err(),
        "a second encode before submit must trip the debug guard"
    );
}

/// A centre-anchored bound number easing from 0 to 100: its content and, as
/// its width changes, its position move every frame until the tween clamps.
fn tweened_health_layer(rig: &mut Rig, now: f64, snap: bool) -> UiDrawData {
    use super::descriptor::{
        AnchoredTree, BindSource, CaptureMode, ColorValue, Easing, TextBind, TextTween, TextWidget,
        Widget,
    };
    let tree = AnchoredTree {
        anchor: super::layout::Anchor::Center,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "0".into(),
            font_size: 24.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: Some(TextBind {
                source: BindSource::Slot {
                    slot: "player.health".into(),
                },
                format: None,
                decimal_places: Some(0),
                tween: Some(TextTween {
                    duration_ms: 200.0,
                    easing: Easing::EaseOut,
                    from: Some(0.0),
                }),
            }),
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
    let entry = postretro_ui::UiTreeEntry {
        name: "hud".into(),
        tier: postretro_ui::modal_stack::ScopeTier::Engine,
        capture_mode: tree.capture_mode,
        descriptor: tree,
        on_commit: None,
    };
    let slots = std::collections::HashMap::from([(
        "player.health".to_string(),
        postretro_entities::SlotValue::Number(100.0),
    )]);
    rig.pass.layout_gameplay_tree(
        &mut rig.font_system,
        0,
        &entry,
        rig.size,
        &super::tree::ImageSizes::new(),
        0,
        &slots,
        &super::tree::CellValues::new(),
        &super::theme::UiTheme::engine_default(),
        0,
        postretro_ui::tree::TweenClock { now, snap },
    )
}

/// O18: once the tween clamps, positions settle bit-identical and the next
/// frame prepares nothing.
#[test]
fn settled_tween_prepares_nothing() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    const DT: f64 = 1.0 / 60.0;
    let mut moving_frames = 0;
    let mut last_prepare = None;
    for i in 0..30 {
        let now = i as f64 * DT;
        let hud = tweened_health_layer(&mut rig, now, false);
        let layers = [UiDrawData::default(), hud];
        let frame = rig.frame(&layers);
        if !frame.prepared.is_empty() {
            moving_frames += 1;
            last_prepare = Some(now);
        }
    }
    assert!(moving_frames > 3, "the tween never moved the text");
    let last = last_prepare.expect("the tween prepared");
    assert!(
        last <= 0.2 + DT,
        "a span still prepared at {last:.3}s, past the 0.2 s tween and one frame"
    );
}

/// Reduce motion snaps a running tween to its target that frame; the next
/// frame prepares nothing.
#[test]
fn reduce_motion_snap_prepares_once_then_nothing() {
    let mut rig = Rig::new(gpu_or_skip!(try_init_gpu()));
    const DT: f64 = 1.0 / 60.0;
    for i in 0..3 {
        let hud = tweened_health_layer(&mut rig, i as f64 * DT, false);
        rig.frame(&[UiDrawData::default(), hud]);
    }
    let snapped = tweened_health_layer(&mut rig, 3.0 * DT, true);
    let snap_frame = rig.frame(&[UiDrawData::default(), snapped]);
    assert_eq!(snap_frame.prepared, vec![(1, 0)], "the snap frame prepares");
    let next = tweened_health_layer(&mut rig, 4.0 * DT, true);
    let after = rig.frame(&[UiDrawData::default(), next]);
    assert_eq!(after.stats, TextPrepareStats::default());
}
