// Frame-level proofs of the specular style and the per-texel specular inputs
// on the compiled `specular-texel-sharpness` dev map, through the capture
// harness's offscreen frames: real id-42 owners and mask, real promoted mover
// specular, and the four-channel fixture materials. Every frame keeps only
// the specular term and the runtime term mover specular rides in (the map has
// no dynamic light, and the style never moves diffuse), so what differs
// between two frames of one fixture is specular.
// Compile the map beside its source (see the map's README) and run with
// `SPECULAR_TEXEL_PRL=<absolute path to specular-texel-sharpness.prl>
// POSTRETRO_SH_STREAMING=sync-proof`, `--features capture -- --ignored`.
// See: context/lib/rendering_pipeline.md §4 (Texel-centre highlight) · §7.8

use glam::{Mat4, Vec3};
use postretro_render_cpu::frame_uniforms::SpecularStyle;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;

use super::prepared::PreparedCapture;
use super::scene::{CaptureScene, parse_scene};
use super::setup::capture_view_projection;
use crate::render::{LightTermMask, Renderer};

const UNIT: f32 = 0.0254;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
/// Frames rendered before any comparison, so streamed SH and the promotion
/// pins have settled; specular reads neither, but the warmup costs little.
const WARMUP: usize = 3;
/// A channel above this carries specular.
const LIT: u8 = 2;
/// Largest channel spread of a white glint, and smallest of a hued one.
const WHITE_SPREAD: u8 = 2;
const HUE_SPREAD: u8 = 16;

/// The compiled dev map. An ignored test run without it fails rather than
/// passing on a skip.
fn map() -> String {
    std::env::var("SPECULAR_TEXEL_PRL")
        .expect("set SPECULAR_TEXEL_PRL to the compiled dev map's absolute path")
}

/// A map-space point (TrenchBroom units, Z up) in engine metres.
fn engine(point: [f32; 3]) -> Vec3 {
    Vec3::new(-point[1], point[2], -point[0]) * UNIT
}

/// One fixture view: a camera, the lights whose mover crossfade it pins at
/// full weight, a static-face rectangle and a mover-face rectangle, each
/// given as two opposite map-space corners on one plane. `marches` is true
/// where the material has a height map and its prefix carries Surface Depth
/// tuning, so the march runs at On.
#[derive(Clone, Copy)]
struct Fixture {
    name: &'static str,
    eye: [f32; 3],
    yaw_deg: f32,
    promoted: &'static [&'static str],
    static_face: [[f32; 3]; 2],
    mover_face: [[f32; 3]; 2],
    marches: bool,
}

/// `_p` without `_h` on a tuned prefix (metal): the ramp wall and
/// `ramp_mover`, under the spot and the point light.
const RAMP: Fixture = Fixture {
    name: "ramp (_p)",
    eye: [576.0, 200.0, 64.0],
    yaw_deg: 90.0,
    promoted: &["point_light", "spot_light"],
    static_face: [[380.0, 512.0, 16.0], [470.0, 512.0, 112.0]],
    mover_face: [[520.0, 416.0, 8.0], [632.0, 416.0, 120.0]],
    marches: false,
};

/// `_t` without `_p`: the inlay wall and `inlay_mover`.
const INLAY: Fixture = Fixture {
    name: "inlay (_t)",
    eye: [480.0, 320.0, 64.0],
    yaw_deg: 0.0,
    promoted: &["point_light", "spot_light"],
    static_face: [[768.0, 110.0, 16.0], [768.0, 200.0, 112.0]],
    mover_face: [[672.0, 264.0, 8.0], [672.0, 376.0, 120.0]],
    marches: false,
};

/// `_p` with `_h` on concrete, the deepest-tuned prefix: the south wall and
/// `carved_door` beside it, under the point light.
const CARVED: Fixture = Fixture {
    name: "carved (_p + _h)",
    eye: [336.0, 220.0, 72.0],
    yaw_deg: 270.0,
    promoted: &["point_light"],
    static_face: [[410.0, 0.0, 16.0], [480.0, 0.0, 128.0]],
    mover_face: [[284.0, 0.0, 8.0], [388.0, 0.0, 136.0]],
    marches: true,
};

/// Legacy one-channel slots (the comparison strip's shipped textures), static
/// only: the strip's dirt and wood panels under the point light.
const STRIP: Fixture = Fixture {
    name: "strip (legacy one-channel)",
    eye: [224.0, 240.0, 96.0],
    yaw_deg: 180.0,
    promoted: &[],
    static_face: [[8.0, 48.0, 40.0], [8.0, 272.0, 152.0]],
    mover_face: [[0.0; 3]; 2],
    marches: false,
};

/// `_p` with `_h` on `glass`, the one prefix whose Surface Depth resolves
/// flat: the glass panel and `glass_mover` beside it on the north wall. The
/// height map carries real relief, so a march that ran would show.
const GLASS: Fixture = Fixture {
    name: "glass (_p + _h, untuned prefix)",
    eye: [96.0, 260.0, 64.0],
    yaw_deg: 90.0,
    promoted: &["point_light", "spot_light"],
    static_face: [[40.0, 504.0, 8.0], [88.0, 504.0, 120.0]],
    mover_face: [[104.0, 504.0, 8.0], [152.0, 504.0, 120.0]],
    marches: false,
};

/// `_p`, `_t` and `_h` together on concrete: the tinted panel and
/// `tinted_mover` beside it on the south wall, seen obliquely from the east
/// near the point light's mirror direction. `_t` alternates 255 and 0 by
/// 4-texel cell column, so each face holds texels at t = 1 and t = 0. No other fixture
/// view sees this pair, so its march never blooms into their rectangles.
const TINTED: Fixture = Fixture {
    name: "tinted (_p + _t + _h)",
    eye: [744.0, 96.0, 64.0],
    yaw_deg: 220.0,
    promoted: &["point_light"],
    static_face: [[584.0, 8.0, 8.0], [632.0, 8.0, 120.0]],
    mover_face: [[648.0, 8.0, 8.0], [696.0, 8.0, 120.0]],
    marches: true,
};

impl Fixture {
    fn scene(&self, style: Option<&str>) -> CaptureScene {
        let eye = engine(self.eye);
        let mut scene = serde_json::json!({
            "map": map(),
            "camera": {
                "position": eye.to_array(),
                "yaw_deg": self.yaw_deg,
                "pitch_deg": 0.0,
                "fov_deg": 90.0
            },
            "resolution": [WIDTH, HEIGHT],
            "output": "unused.png",
            "force_full_resident_sh_compose": true,
            "reflection_quality": "off",
            // Specular plus the runtime term promoted mover specular rides
            // in; the map has no dynamic light, so world faces keep only
            // specular, and the mover's runtime diffuse ignores the style.
            "light_term_mask": LightTermMask::SPECULAR.bits() | LightTermMask::DYNAMIC_DIRECT.bits(),
            "force_promotion": self
                .promoted
                .iter()
                .map(|tag| serde_json::json!({ "tag": tag, "weight": 1.0 }))
                .collect::<Vec<_>>(),
        });
        if let Some(style) = style {
            scene["specular_style"] = serde_json::json!(style);
        }
        parse_scene(&scene.to_string()).expect("fixture scene parses")
    }

    fn prepare(&self, style: Option<&str>) -> Capture {
        self.prepare_with(style, |_| {})
    }

    fn prepare_with(
        &self,
        style: Option<&str>,
        before_install: impl FnOnce(&mut Renderer),
    ) -> Capture {
        let scene = self.scene(style);
        let view_proj = capture_view_projection(&scene.camera, WIDTH, HEIGHT);
        let prepared = PreparedCapture::prepare_with(&scene, before_install)
            .unwrap_or_else(|error| panic!("{}: capture prepares: {error:#}", self.name));
        Capture {
            prepared,
            static_rect: rect(view_proj, self.static_face),
            mover_rect: (!self.promoted.is_empty()).then(|| rect(view_proj, self.mover_face)),
            name: self.name,
        }
    }
}

/// Pixel bounds `[x0, x1) × [y0, y1)` of a map-space rectangle, inset by two
/// pixels so a filtered edge never counts.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: [u32; 2],
    y: [u32; 2],
}

fn rect(view_proj: Mat4, [low, high]: [[f32; 3]; 2]) -> Rect {
    // The two corners span one axis-aligned plane: one coordinate is shared.
    let corners: Vec<[f32; 3]> = (0..4)
        .map(|index| {
            let mut corner = low;
            let mut varying = (0..3).filter(|&axis| low[axis] != high[axis]);
            let (a, b) = (varying.next().unwrap(), varying.next().unwrap());
            if index & 1 != 0 {
                corner[a] = high[a];
            }
            if index & 2 != 0 {
                corner[b] = high[b];
            }
            corner
        })
        .collect();
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for corner in corners {
        let clip = view_proj * engine(corner).extend(1.0);
        assert!(
            clip.w > 0.0,
            "fixture rectangle lies in front of the camera"
        );
        let ndc = clip.truncate() / clip.w;
        let px = (ndc.x + 1.0) * 0.5 * WIDTH as f32;
        let py = (1.0 - ndc.y) * 0.5 * HEIGHT as f32;
        x0 = x0.min(px);
        x1 = x1.max(px);
        y0 = y0.min(py);
        y1 = y1.max(py);
    }
    let clamp = |value: f32, limit: u32| value.clamp(0.0, limit as f32) as u32;
    let rect = Rect {
        x: [clamp(x0 + 2.0, WIDTH), clamp(x1 - 2.0, WIDTH)],
        y: [clamp(y0 + 2.0, HEIGHT), clamp(y1 - 2.0, HEIGHT)],
    };
    assert!(
        rect.x[1] > rect.x[0] + 4 && rect.y[1] > rect.y[0] + 4,
        "fixture rectangle is on screen: {rect:?}"
    );
    rect
}

struct Capture {
    prepared: PreparedCapture,
    static_rect: Rect,
    mover_rect: Option<Rect>,
    name: &'static str,
}

impl Capture {
    fn frame(&mut self) -> Vec<u8> {
        self.prepared.capture_frame().expect("frame captures")
    }

    /// A settled frame: warm up, then require two consecutive frames to
    /// match, so every later comparison is against a deterministic frame.
    fn settled(&mut self) -> Vec<u8> {
        for _ in 0..WARMUP {
            self.frame();
        }
        let first = self.frame();
        let second = self.frame();
        assert!(
            first == second,
            "{}: consecutive frames must match before comparing",
            self.name
        );
        second
    }

    fn renderer(&mut self) -> &mut Renderer {
        self.prepared.renderer_mut()
    }

    /// Pixels whose rgb differs between `a` and `b` inside `rect`.
    fn changed(a: &[u8], b: &[u8], rect: Rect) -> usize {
        let mut count = 0;
        for y in rect.y[0]..rect.y[1] {
            for x in rect.x[0]..rect.x[1] {
                let at = ((y * WIDTH + x) * 4) as usize;
                if a[at..at + 3] != b[at..at + 3] {
                    count += 1;
                }
            }
        }
        count
    }

    /// Pixels with any specular (non-black rgb) inside `rect`.
    fn lit(frame: &[u8], rect: Rect) -> usize {
        let mut count = 0;
        for y in rect.y[0]..rect.y[1] {
            for x in rect.x[0]..rect.x[1] {
                let at = ((y * WIDTH + x) * 4) as usize;
                if frame[at..at + 3].iter().any(|&channel| channel > LIT) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Lit pixels inside `rect` that are white (equal channels) and that
    /// carry a hue. A static face keeps only specular under a white light, so
    /// a `_t` texel at t = 0 glints white and one at t = 1 in its albedo's
    /// hue.
    fn white_and_hued(frame: &[u8], rect: Rect) -> (usize, usize) {
        let (mut white, mut hued) = (0, 0);
        for y in rect.y[0]..rect.y[1] {
            for x in rect.x[0]..rect.x[1] {
                let at = ((y * WIDTH + x) * 4) as usize;
                let rgb = &frame[at..at + 3];
                let (low, high) = (*rgb.iter().min().unwrap(), *rgb.iter().max().unwrap());
                if high <= LIT {
                    continue;
                }
                if high - low <= WHITE_SPREAD {
                    white += 1;
                } else if high - low >= HUE_SPREAD {
                    hued += 1;
                }
            }
        }
        (white, hued)
    }

    /// The style alters both the static face and the mover face between
    /// `pixel` and `smooth`, each of which carries specular.
    fn assert_style_alters_both_faces(&self, pixel: &[u8], smooth: &[u8], what: &str) {
        let name = self.name;
        let mut rects = vec![("static face", self.static_rect)];
        rects.extend(self.mover_rect.map(|rect| ("mover face", rect)));
        for (face, rect) in rects {
            assert!(
                Self::lit(pixel, rect) > 0 && Self::lit(smooth, rect) > 0,
                "{name}: {what}: the {face} carries specular"
            );
            let changed = Self::changed(pixel, smooth, rect);
            eprintln!("[SpecularFrame] {name}: {what}: {changed} {face} pixels change");
            assert!(changed > 0, "{name}: {what}: the style alters the {face}");
        }
    }
}

/// With `SPECULAR_FRAME_DUMP` set to a directory, write `frame` there as a
/// PNG named `label`, for inspecting a failing comparison.
fn dump(label: &str, frame: &[u8]) {
    let Ok(dir) = std::env::var("SPECULAR_FRAME_DUMP") else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{label}.png"));
    image::RgbaImage::from_raw(WIDTH, HEIGHT, frame.to_vec())
        .expect("an RGBA frame")
        .save(&path)
        .expect("the dump writes");
}

// AC 13, pixel half: on the static faces and movers of every fixture, a live
// style change alters the next frame's specular, and changing back restores
// the earlier frame byte for byte. (No pipeline or bind group is built: the
// renderer's `a_live_specular_style_change_reaches_the_next_frame_without_a_rebuild`.)
#[test]
#[ignore = "requires a GPU adapter and SPECULAR_TEXEL_PRL (specular-texel-sharpness compiled under content/dev/maps); run under POSTRETRO_SH_STREAMING=sync-proof"]
fn dev_map_live_specular_style_change_alters_the_next_frame_on_static_faces_and_movers() {
    for fixture in [RAMP, INLAY, CARVED] {
        let mut capture = fixture.prepare(None);
        let pixel = capture.settled();
        capture.renderer().set_specular_style(SpecularStyle::Smooth);
        let smooth = capture.frame();
        let label = fixture.name.split(' ').next().unwrap_or("fixture");
        eprintln!(
            "[SpecularFrame] {label}: static {:?}, mover {:?}",
            capture.static_rect, capture.mover_rect
        );
        dump(&format!("{label}-pixel"), &pixel);
        dump(&format!("{label}-smooth"), &smooth);
        capture.assert_style_alters_both_faces(&pixel, &smooth, "pixel → smooth, next frame");
        capture.renderer().set_specular_style(SpecularStyle::Pixel);
        assert!(
            capture.frame() == pixel,
            "{}: changing back restores the pixel frame",
            fixture.name
        );
    }
}

// AC 11 (pin P4), pixel half: an install's first frame draws the style
// committed before the install (here, capture's scene key, applied before the
// level installs), equal to the frame a live change reaches.
#[test]
#[ignore = "requires a GPU adapter and SPECULAR_TEXEL_PRL (specular-texel-sharpness compiled under content/dev/maps); run under POSTRETRO_SH_STREAMING=sync-proof"]
fn dev_map_specular_style_reaches_the_first_frame_of_an_install() {
    for fixture in [RAMP, INLAY, CARVED] {
        let mut smooth_install = fixture.prepare(Some("smooth"));
        let first_smooth = smooth_install.frame();
        let mut pixel_install = fixture.prepare(Some("pixel"));
        let first_pixel = pixel_install.frame();
        smooth_install.assert_style_alters_both_faces(
            &first_pixel,
            &first_smooth,
            "first frame of each install",
        );
        pixel_install.settled();
        pixel_install
            .renderer()
            .set_specular_style(SpecularStyle::Smooth);
        let live = pixel_install.settled();
        let settled_smooth = smooth_install.settled();
        assert!(
            live == settled_smooth,
            "{}: the committed-before-install style draws as a live change does",
            fixture.name
        );
        assert!(
            first_smooth == settled_smooth,
            "{}: the install's first frame already draws the committed style",
            fixture.name
        );
    }
}

// AC 15 (pins P1, P2): style and Surface Depth switch independently. On the
// carved fixture, under each order of changes, every revisit of a (style,
// Surface Depth) state renders the frame its first visit did: smooth survives
// Off then On, the march is restored at On, and a style change at Off leaves
// the march off.
#[test]
#[ignore = "requires a GPU adapter and SPECULAR_TEXEL_PRL (specular-texel-sharpness compiled under content/dev/maps); run under POSTRETRO_SH_STREAMING=sync-proof"]
fn specular_style_and_surface_depth_switch_independently() {
    let mut capture = CARVED.prepare(None);
    let visit = |capture: &mut Capture, style: SpecularStyle, depth: SurfaceDepthQuality| {
        capture.renderer().set_specular_style(style);
        capture.renderer().set_surface_depth_quality(depth);
        capture.settled()
    };
    use SpecularStyle::{Pixel, Smooth};
    use SurfaceDepthQuality::{Off, On};
    let pixel_on = visit(&mut capture, Pixel, On);
    // P1: smooth committed, then Surface Depth Off, then On.
    let smooth_on = visit(&mut capture, Smooth, On);
    let smooth_off = visit(&mut capture, Smooth, Off);
    let smooth_on_again = visit(&mut capture, Smooth, On);
    // P2: Surface Depth Off, then smooth, then pixel.
    let pixel_off = visit(&mut capture, Pixel, Off);
    let smooth_off_again = visit(&mut capture, Smooth, Off);
    let pixel_off_again = visit(&mut capture, Pixel, Off);
    let pixel_on_again = visit(&mut capture, Pixel, On);

    let rect = capture.static_rect;
    let door = capture.mover_rect.expect("the carved door");
    for (face, rect) in [("wall", rect), ("door", door)] {
        // The march is live on this fixture: Off and On differ at each style.
        assert!(
            Capture::changed(&smooth_on, &smooth_off, rect) > 0,
            "{face}: march at smooth"
        );
        assert!(
            Capture::changed(&pixel_on, &pixel_off, rect) > 0,
            "{face}: march at pixel"
        );
        // The style is live at each Surface Depth state.
        assert!(
            Capture::changed(&pixel_on, &smooth_on, rect) > 0,
            "{face}: style at On"
        );
        assert!(
            Capture::changed(&pixel_off, &smooth_off, rect) > 0,
            "{face}: style at Off"
        );
    }
    assert!(
        smooth_on_again == smooth_on,
        "P1: smooth kept and the march restored"
    );
    assert!(
        smooth_off_again == smooth_off,
        "P2: smooth at Off, march still off"
    );
    assert!(
        pixel_off_again == pixel_off,
        "P2: pixel at Off, march still off"
    );
    assert!(pixel_on_again == pixel_on, "the march returns at On");
}

// AC 20 (pin P3) and AC 21 (pin P11), frame half: per-texel specular inputs
// live in the slot's bytes, never in the material uniform Surface Depth
// rewrites. A level whose materials build at Surface Depth Off renders, at
// Off and then at On, exactly the frames of a level built at On and switched
// Off then On, on the static faces and movers of `_p` (ramp), `_t` (inlay),
// `_p` + `_h` (carved), `_p` + `_h` on the untuned glass prefix, `_p` + `_t`
// + `_h` (tinted) and a legacy one-channel slot (strip), under each style.
// The march runs only where a height map meets a tuned prefix (carved,
// tinted): there Off and On differ. Everywhere else Off and On render alike,
// glass included, whose height map carries relief its flat prefix never
// marches. On the tinted panel, t = 1 texels glint in their albedo's hue and
// t = 0 texels white in every state.
#[test]
#[ignore = "requires a GPU adapter and SPECULAR_TEXEL_PRL (specular-texel-sharpness compiled under content/dev/maps); run under POSTRETRO_SH_STREAMING=sync-proof"]
fn dev_map_per_texel_specular_holds_through_surface_depth_off_and_on() {
    for fixture in [RAMP, INLAY, CARVED, STRIP, GLASS, TINTED] {
        for style in SpecularStyle::ALL {
            let mut built_on = fixture.prepare(None);
            built_on.renderer().set_specular_style(style);
            let on = built_on.settled();
            built_on
                .renderer()
                .set_surface_depth_quality(SurfaceDepthQuality::Off);
            let on_then_off = built_on.settled();
            built_on
                .renderer()
                .set_surface_depth_quality(SurfaceDepthQuality::On);
            let on_off_on = built_on.settled();

            let mut built_off = fixture.prepare_with(None, |renderer| {
                renderer.set_surface_depth_quality(SurfaceDepthQuality::Off);
            });
            built_off.renderer().set_specular_style(style);
            let off = built_off.settled();
            built_off
                .renderer()
                .set_surface_depth_quality(SurfaceDepthQuality::On);
            let off_then_on = built_off.settled();

            let name = format!("{} {style:?}", fixture.name);
            let label = format!(
                "{}-{style:?}",
                fixture.name.split(' ').next().unwrap_or("fixture")
            );
            dump(&format!("{label}-on"), &on);
            dump(&format!("{label}-off"), &off);
            let mut rects = vec![built_on.static_rect];
            rects.extend(built_on.mover_rect);
            for &rect in &rects {
                assert!(Capture::lit(&on, rect) > 0, "{name}: specular");
            }
            assert!(
                off == on_then_off,
                "{name}: built at Off draws as switched to Off"
            );
            assert!(
                off_then_on == on,
                "{name}: built at Off, then On, draws as built at On"
            );
            assert!(on_off_on == on, "{name}: Off then On restores the frame");
            for &rect in &rects {
                let changed = Capture::changed(&on, &off, rect);
                if fixture.marches {
                    assert!(changed > 0, "{name}: `_h` marches at On only");
                } else {
                    assert_eq!(
                        changed, 0,
                        "{name}: no march (no height map, or a flat prefix): Off draws as On"
                    );
                }
            }
            if fixture.name == TINTED.name {
                for (state, frame) in [("On", &on), ("Off", &off), ("Off then On", &off_then_on)] {
                    let (white, hued) = Capture::white_and_hued(frame, built_on.static_rect);
                    eprintln!("[SpecularFrame] {name} at {state}: {white} white, {hued} hued");
                    assert!(
                        white > 0 && hued > 0,
                        "{name} at {state}: t = 0 texels glint white and t = 1 texels in their hue"
                    );
                }
            }
        }
    }
}
