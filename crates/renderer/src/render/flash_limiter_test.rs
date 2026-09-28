// GPU tests for the photosensitivity flash limiter: synthetic `scene_color`
// and effect-slot sequences through the channel clamp, the measure pass and the
// resolve into an offscreen target, read back every frame and counted by an
// independent WCAG counter. A run with no adapter skips; a skip is not a pass.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use std::collections::HashMap;

use postretro_entities::SlotValue;
use postretro_render_cpu::flash_limiter::{
    FLASH_LIMITER_SLOT, HITCH_CEILING_SECONDS, LimiterFrameInput, SplashHandOff,
};

use super::gpu_test_harness::{GpuCtx, read_texture_rgba8, try_init_gpu};
use super::screen_effects::{ResolveTimestamps, ScreenEffectsPass};
use super::wcag_flash_counter::{
    RedTransitionCounter, TransitionCounter, max_transitions_in_any_second, srgb8_to_linear,
};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const PRESENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// The splash the hand-off tests present: near black.
const SPLASH_RGB: [f32; 3] = [0.012, 0.015, 0.02];

/// A rectangle of the synthetic scene, in pixels.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

const FULL: Rect = Rect {
    x: 0,
    y: 0,
    w: WIDTH,
    h: HEIGHT,
};

impl Rect {
    fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// A rectangle of whole limiter cells (10×10 pixels at 160×90).
fn cells(x: u32, y: u32, w: u32, h: u32) -> Rect {
    let cell = WIDTH / 16;
    Rect {
        x: x * cell,
        y: y * cell,
        w: w * cell,
        h: h * cell,
    }
}

/// One synthetic `scene_color` frame: a black field with `rect` at `rgb`.
fn scene(rect: Rect, rgb: [f32; 3]) -> Vec<u8> {
    scene_of(&[(rect, rgb)])
}

/// A black field with each rectangle painted in order, later ones on top.
fn scene_of(rects: &[(Rect, [f32; 3])]) -> Vec<u8> {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 8) as usize);
    let lit: Vec<[u16; 3]> = rects
        .iter()
        .map(|(_, rgb)| rgb.map(f32_to_f16_bits))
        .collect();
    let one = f32_to_f16_bits(1.0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut px = [0; 3];
            for ((rect, _), color) in rects.iter().zip(&lit) {
                if rect.contains(x, y) {
                    px = *color;
                }
            }
            for half in [px[0], px[1], px[2], one] {
                data.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    data
}

fn gray(level: f32) -> [f32; 3] {
    [level; 3]
}

/// Round-to-nearest f32 → IEEE half bits for the non-negative, in-range values
/// these fixtures use.
fn f32_to_f16_bits(value: f32) -> u16 {
    if value <= 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    if exp <= 0 {
        return 0;
    }
    let mantissa = bits & 0x7f_ffff;
    let half = ((exp as u32) << 10) | (mantissa >> 13);
    let round = (mantissa >> 12) & 1;
    (half + round) as u16
}

/// One presented frame's input.
struct Frame {
    pixels: Vec<u8>,
    slots: HashMap<String, SlotValue>,
    elapsed: f32,
    splash: Option<f32>,
}

impl Frame {
    fn scene(pixels: Vec<u8>, elapsed: f32) -> Self {
        Self {
            pixels,
            slots: HashMap::new(),
            elapsed,
            splash: None,
        }
    }

    fn slot(mut self, name: &str, value: SlotValue) -> Self {
        self.slots.insert(name.to_string(), value);
        self
    }

    fn limiter_off(self) -> Self {
        self.slot(FLASH_LIMITER_SLOT, SlotValue::Boolean(false))
    }

    fn flash(self, rgb: [f32; 3], a: f32) -> Self {
        self.slot(
            "screen.flash",
            SlotValue::Array(vec![rgb[0], rgb[1], rgb[2], a]),
        )
    }

    /// Splash frames presented since the previous resolve frame, for `seconds`.
    fn after_splash(mut self, seconds: f32) -> Self {
        self.splash = Some(seconds);
        self
    }
}

struct Rig {
    ctx: GpuCtx,
    pass: ScreenEffectsPass,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    size: [u32; 2],
}

impl Rig {
    fn new() -> Option<Self> {
        let ctx = try_init_gpu()?;
        let pass = ScreenEffectsPass::new(&ctx.device, WIDTH, HEIGHT, PRESENT_FORMAT);
        let (target, target_view) = Self::target(&ctx, WIDTH, HEIGHT);
        Some(Self {
            ctx,
            pass,
            target,
            target_view,
            size: [WIDTH, HEIGHT],
        })
    }

    fn target(ctx: &GpuCtx, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Flash limiter test target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PRESENT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        (target, view)
    }

    /// Resize the scene target and the presented target; the limiter keeps its
    /// history. Frames tile the 160×90 fixture over the new size, so the
    /// top-left tile measures the same content at any size.
    fn resize(&mut self, width: u32, height: u32) {
        self.pass.resize(&self.ctx.device, width, height);
        let (target, view) = Self::target(&self.ctx, width, height);
        self.target = target;
        self.target_view = view;
        self.size = [width, height];
    }

    fn upload(&self, pixels: &[u8]) {
        let [w, h] = self.size;
        let mut data = Vec::with_capacity((w * h * 8) as usize);
        for y in 0..h {
            for x in 0..w {
                let i = (((y % HEIGHT) * WIDTH + (x % WIDTH)) * 8) as usize;
                data.extend_from_slice(&pixels[i..i + 8]);
            }
        }
        self.ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.pass.scene_color_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Present one frame and return the read-back RGBA8 bytes.
    fn present(&mut self, frame: &Frame) -> Presented {
        self.upload(&frame.pixels);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.pass.encode_resolve(
            &self.ctx.queue,
            &mut encoder,
            &self.target_view,
            &frame.slots,
            LimiterFrameInput {
                elapsed_seconds: frame.elapsed,
                splash: frame.splash.map(|seconds| SplashHandOff {
                    rgb: SPLASH_RGB,
                    seconds,
                }),
            },
            ResolveTimestamps::default(),
        );
        let [w, h] = self.size;
        Presented {
            bytes: read_texture_rgba8(&self.ctx, &self.target, w, h, encoder).pixels,
            width: w,
        }
    }

    fn run(&mut self, frames: impl IntoIterator<Item = Frame>) -> Vec<Presented> {
        frames
            .into_iter()
            .map(|frame| self.present(&frame))
            .collect()
    }

    /// The capture tonemap of `pixels`, read back.
    fn capture(&mut self, pixels: &[u8]) -> Vec<u8> {
        self.upload(pixels);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let [w, h] = self.size;
        let texture =
            self.pass
                .encode_capture_tonemap(&self.ctx.device, &self.ctx.queue, &mut encoder, w, h);
        read_texture_rgba8(&self.ctx, &texture, w, h, encoder).pixels
    }
}

struct Presented {
    bytes: Vec<u8>,
    width: u32,
}

impl Presented {
    fn mean_rgb(&self, rect: Rect) -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        for y in rect.y..rect.y + rect.h {
            for x in rect.x..rect.x + rect.w {
                let i = ((y * self.width + x) * 4) as usize;
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += srgb8_to_linear(self.bytes[i + c]);
                }
            }
        }
        sum.map(|s| s / (rect.w * rect.h) as f32)
    }

    fn luminance(&self, rect: Rect) -> f32 {
        let rgb = self.mean_rgb(rect);
        rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
    }
}

macro_rules! rig_or_skip {
    () => {
        match Rig::new() {
            Some(rig) => rig,
            None => {
                eprintln!("flash limiter GPU test: no adapter, skipping (not a pass)");
                return;
            }
        }
    };
}

/// Square-wave strobe level at time `t`: dark for the first half of each
/// period, so every flash is a brightening from a dark rest.
fn square(t: f32, hz: f32) -> f32 {
    if (t * hz).fract() < 0.5 { 0.0 } else { 1.0 }
}

/// Smooth strobe between 0 and 1 starting dark.
fn sine(t: f32, hz: f32) -> f32 {
    0.5 - 0.5 * (t * hz * std::f32::consts::TAU).cos()
}

/// A `seconds`-long scene strobe of `rect` at `fps`, `level(t)` per frame.
fn strobe(rect: Rect, fps: f32, seconds: f32, level: impl Fn(f32) -> f32) -> Vec<Frame> {
    let dt = 1.0 / fps;
    (0..(seconds * fps).round() as usize)
        .map(|n| Frame::scene(scene(rect, gray(level(n as f32 * dt))), dt))
        .collect()
}

/// Worst transitions in any one second of `presented` over `rect`.
fn worst(presented: &[Presented], fps: f32, rect: Rect) -> usize {
    let mut counter = TransitionCounter::default();
    for (n, frame) in presented.iter().enumerate() {
        counter.push(n as f32 / fps, frame.luminance(rect));
    }
    max_transitions_in_any_second(counter.transition_times())
}

fn quarter() -> Rect {
    Rect {
        x: 0,
        y: 0,
        w: WIDTH / 2,
        h: HEIGHT / 2,
    }
}

#[test]
fn full_screen_square_strobe_is_held_to_three_flashes_per_second() {
    let mut rig = rig_or_skip!();
    let fps = 60.0;
    let presented = rig.run(strobe(FULL, fps, 3.0, |t| square(t, 5.0)));
    let worst = worst(&presented, fps, FULL);
    assert!(worst <= 6, "{worst} transitions in one second");
    assert!(
        worst >= 4,
        "the limited strobe still shows flashes: {worst}"
    );
    // Over budget, the strobe rests at its pre-flash (dark) level: a whole
    // flash period presents no brightening.
    let period = (fps / 5.0) as usize;
    assert!(
        presented[..fps as usize]
            .windows(period)
            .any(|w| w.iter().all(|p| p.luminance(FULL) < 0.1))
    );
}

#[test]
fn four_flashes_in_a_second_show_three_and_three_all_show() {
    let fps = 60.0;
    // Flashes 1/8 s long, one every 1/4 s.
    let flashes = |count: usize| {
        move |t: f32| {
            let n = (t * 4.0) as usize;
            if n < count && (t * 4.0).fract() >= 0.5 {
                1.0
            } else {
                0.0
            }
        }
    };
    let mut four = rig_or_skip!();
    let presented = four.run(strobe(FULL, fps, 2.0, flashes(4)));
    assert_eq!(
        worst(&presented, fps, FULL),
        6,
        "three flashes, each a rise and a return"
    );
    assert!(
        presented.last().unwrap().luminance(FULL) < 0.02,
        "ends at its pre-flash level"
    );

    let mut three = rig_or_skip!();
    let presented = three.run(strobe(FULL, fps, 2.0, flashes(3)));
    assert_eq!(worst(&presented, fps, FULL), 6, "all three appear");
}

#[test]
fn the_same_strobe_limits_the_same_at_30_and_240_hz() {
    // L2 and hub AC 4: square and sine, 5 Hz.
    for shape in [square as fn(f32, f32) -> f32, sine] {
        let mut counts = Vec::new();
        for fps in [30.0, 240.0] {
            let mut rig = rig_or_skip!();
            let presented = rig.run(strobe(FULL, fps, 2.0, |t| shape(t, 5.0)));
            counts.push(worst(&presented, fps, FULL));
        }
        assert!(counts.iter().all(|&c| c <= 6), "{counts:?}");
        assert_eq!(counts[0], counts[1], "same count at 30 and 240 Hz");
    }
}

#[test]
fn a_strobe_alternating_every_frame_never_presents_faster_than_the_cap() {
    let mut rig = rig_or_skip!();
    let fps = 60.0;
    let presented = rig.run(strobe(FULL, fps, 1.0, |t| {
        ((t * fps).round() as u32 % 2) as f32
    }));
    let cap = 4.0 / fps;
    for pair in presented.windows(2) {
        let step = (pair[1].luminance(FULL) - pair[0].luminance(FULL)).abs();
        assert!(step <= cap + 0.01, "step {step} over the {cap} cap");
    }
}

/// Seconds for a black→white step to reach 95% at `fps`.
fn ramp_seconds(fps: f32) -> Option<f32> {
    let mut rig = Rig::new()?;
    let dt = 1.0 / fps;
    rig.present(&Frame::scene(scene(FULL, gray(0.0)), dt));
    for n in 1..(fps as usize * 2) {
        if rig
            .present(&Frame::scene(scene(FULL, gray(1.0)), dt))
            .luminance(FULL)
            >= 0.95
        {
            return Some(n as f32 * dt);
        }
    }
    None
}

#[test]
fn a_full_screen_change_faster_than_the_cap_takes_the_same_time_at_any_rate() {
    let (Some(at_30), Some(at_240), Some(at_20)) =
        (ramp_seconds(30.0), ramp_seconds(240.0), ramp_seconds(20.0))
    else {
        eprintln!("flash limiter GPU test: no adapter, skipping (not a pass)");
        return;
    };
    assert!((0.2..=0.3).contains(&at_30), "{at_30}");
    assert!(
        (at_30 - at_240).abs() <= 1.0 / 30.0 + 1e-3,
        "{at_30} vs {at_240}"
    );
    // L3: at a steady 20 fps each frame is a hitch; black to white takes at
    // least 375 ms.
    assert!(at_20 >= 0.375, "{at_20}");
}

#[test]
fn a_hitch_never_turns_the_cap_into_one_step_and_a_slow_change_passes() {
    let mut rig = rig_or_skip!();
    let before = rig.present(&Frame::scene(scene(FULL, gray(0.0)), 1.0 / 60.0));
    let after = rig.present(&Frame::scene(scene(FULL, gray(1.0)), 2.0));
    let step = after.luminance(FULL) - before.luminance(FULL);
    assert!(step <= 4.0 * HITCH_CEILING_SECONDS + 0.01, "{step}");

    // A 2-per-second ramp is under the cap: presented as authored.
    let mut limited = rig_or_skip!();
    let mut unlimited = rig_or_skip!();
    let ramp = |rig: &mut Rig, off: bool| {
        (0..30)
            .map(|n| {
                let frame = Frame::scene(scene(FULL, gray(n as f32 / 30.0)), 1.0 / 60.0);
                rig.present(&if off { frame.limiter_off() } else { frame })
                    .bytes
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(ramp(&mut limited, false), ramp(&mut unlimited, true));
}

#[test]
fn turning_the_limiter_back_on_starts_a_fresh_window_and_presents_that_frame_unchanged() {
    // L8 (UO5, restated by owner decision A): on, off mid-strobe for about
    // half a second, then on again.
    let fps = 60.0;
    let dt = 1.0 / fps;
    // A 5 Hz strobe starting dark: its sixth limited transition, the third
    // fall, counts on frame 37, so the window is full when the limiter turns
    // off. It turns back on at frame 66 (1.1 s), while all six are still under
    // a second old.
    let off = 38..66;
    let enable = off.end;
    let level = |n: usize| -> f32 {
        if n < enable {
            return square(n as f32 * dt, 5.0);
        }
        // From the enabling frame: one bright frame, four dark, then a 5 Hz
        // strobe starting bright, all at 0.7.
        let k = n - enable;
        let on = k == 0 || (k >= 5 && ((k - 5) / 6) % 2 == 0);
        if on { 0.7 } else { 0.0 }
    };
    let mut rig = rig_or_skip!();
    let presented: Vec<Presented> = (0..enable + 60)
        .map(|n| {
            let frame = Frame::scene(scene(FULL, gray(level(n))), dt);
            rig.present(&if off.contains(&n) {
                frame.limiter_off()
            } else {
                frame
            })
        })
        .collect();
    // The enabling frame jumps from dark and presents unchanged: it starts
    // fresh against its own measure, with nothing to rate-cap from.
    assert!(presented[enable - 1].luminance(FULL) < 0.02);
    let first = presented[enable].luminance(FULL);
    assert!((first - 0.7).abs() < 0.02, "{first}");
    // No transition from before the off counts: the fall that follows and the
    // next rise are both admitted. With the earlier six still in the window,
    // that rise would be held at its trough.
    let lum: Vec<f32> = presented[enable..]
        .iter()
        .map(|p| p.luminance(FULL))
        .collect();
    let trough = lum[1..=5].iter().copied().fold(f32::INFINITY, f32::min);
    let peak = lum[5..=10].iter().copied().fold(0.0, f32::max);
    assert!(trough < 0.6, "the fall is admitted: {:?}", &lum[..=10]);
    assert!(
        peak >= trough + 0.2,
        "the next rise is admitted: {:?}",
        &lum[..=10]
    );
    let after = worst(&presented[enable..], fps, FULL);
    assert!(after <= 6, "{after}");
}

#[test]
fn a_resize_mid_strobe_keeps_the_budget() {
    let fps = 60.0;
    let mut rig = rig_or_skip!();
    let frames = strobe(FULL, fps, 2.0, |t| square(t, 5.0));
    let mut presented = Vec::new();
    for (n, frame) in frames.into_iter().enumerate() {
        if n % 15 == 7 {
            // A drag: a new size every quarter second.
            let grow = 1 + (n / 15) as u32 % 2;
            rig.resize(WIDTH * grow, HEIGHT * grow);
        }
        presented.push(rig.present(&frame));
    }
    // The top-left 160×90 tile carries the fixture at any size.
    assert!(worst(&presented, fps, FULL) <= 6);
}

#[test]
fn a_screen_flash_strobe_and_a_scene_strobe_share_one_budget() {
    // Hub AC 4: each at two flashes per second, out of phase.
    let fps = 60.0;
    let mut rig = rig_or_skip!();
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| {
            let t = n as f32 / fps;
            let scene_on = square(t, 2.0);
            let flash_on = square(t + 0.25, 2.0);
            Frame::scene(scene(FULL, gray(scene_on)), 1.0 / fps).flash([1.0; 3], flash_on)
        })
        .collect();
    let presented = rig.run(frames);
    assert!(worst(&presented, fps, FULL) <= 6);
}

#[test]
fn the_channel_clamp_alone_packs_a_four_flash_screen_flash_strobe_as_three() {
    let fps = 60.0;
    let mut rig = rig_or_skip!();
    rig.pass.bypass_frame_limiter = true;
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| {
            let t = n as f32 / fps;
            Frame::scene(scene(FULL, gray(0.0)), 1.0 / fps).flash([1.0; 3], square(t, 4.0))
        })
        .collect();
    let presented = rig.run(frames);
    assert!(worst(&presented, fps, FULL) <= 6);
}

#[test]
fn off_both_stages_pass_content_as_a_resolve_with_no_limiter_stage() {
    let fps = 60.0;
    let frames = || -> Vec<Frame> {
        (0..60)
            .map(|n| {
                let t = n as f32 / fps;
                Frame::scene(scene(FULL, gray(square(t, 8.0))), 1.0 / fps)
                    .flash([1.0, 0.0, 0.0], square(t + 0.1, 8.0))
                    .slot(
                        "screen.vignette",
                        SlotValue::Array(vec![1.0, 0.0, 0.0, square(t, 6.0)]),
                    )
            })
            .collect()
    };
    let mut off = rig_or_skip!();
    let mut bypassed = rig_or_skip!();
    bypassed.pass.bypass_channel_clamp = true;
    bypassed.pass.bypass_frame_limiter = true;
    for (a, b) in frames().into_iter().zip(frames()) {
        let off_frame = off.present(&a.limiter_off());
        let packed = off.pass.last_packed;
        let bypassed_frame = bypassed.present(&b);
        assert_eq!(off_frame.bytes, bypassed_frame.bytes);
        assert_eq!(
            packed, bypassed.pass.last_packed,
            "screen effects reach the resolve as authored"
        );
    }
}

#[test]
fn both_stages_present_the_frame_limiter_over_the_clamps_recorded_output() {
    // UO4: clamp → uniform → measure → resolve. Replaying the clamp's packed
    // output with the clamp bypassed presents the same frames, so a flash the
    // clamp suppressed used none of the frame limiter's budget.
    let fps = 60.0;
    let frames = || -> Vec<Frame> {
        (0..(1.5 * fps) as usize)
            .map(|n| {
                let t = n as f32 / fps;
                Frame::scene(scene(FULL, gray(0.05)), 1.0 / fps).flash([1.0; 3], square(t, 4.0))
            })
            .collect()
    };
    let mut both = rig_or_skip!();
    let mut replay = rig_or_skip!();
    replay.pass.bypass_channel_clamp = true;
    for frame in frames() {
        let presented = both.present(&frame);
        let flash = both.pass.last_packed.flash;
        let replayed = replay.present(&Frame {
            slots: HashMap::from([("screen.flash".to_string(), SlotValue::Array(flash.to_vec()))]),
            ..frame
        });
        assert_eq!(presented.bytes, replayed.bytes);
    }
}

#[test]
fn a_splash_stretch_or_hitch_ages_out_earlier_transitions() {
    // UO6: after 2 s, no earlier transition counts, while the frame's own
    // intensity change stays within one hitch allowance.
    let fps = 60.0;
    for splash in [false, true] {
        let mut rig = rig_or_skip!();
        rig.run(strobe(FULL, fps, 1.0, |t| square(t, 5.0)));
        let dark = rig.present(&Frame::scene(scene(FULL, gray(0.0)), 1.0 / fps));
        let gap = Frame::scene(scene(FULL, gray(1.0)), 2.0);
        let first = rig.present(&if splash { gap.after_splash(2.0) } else { gap });
        let from = if splash { 0.015 } else { dark.luminance(FULL) };
        let step = first.luminance(FULL) - from;
        assert!(step <= 4.0 * HITCH_CEILING_SECONDS + 0.02, "{step}");
        // A fresh budget: the next strobe shows its full three flashes.
        let after = rig.run(strobe(FULL, fps, 1.0, |t| square(t, 5.0)));
        assert!(
            (4..=6).contains(&worst(&after, fps, FULL)),
            "splash {splash}"
        );
    }
}

#[test]
fn the_limiter_runs_when_its_flag_is_absent_or_malformed() {
    let fps = 60.0;
    for flag in [
        None,
        Some(SlotValue::Number(0.0)),
        Some(SlotValue::String("off".into())),
    ] {
        let mut rig = rig_or_skip!();
        let frames =
            strobe(FULL, fps, 2.0, |t| square(t, 5.0))
                .into_iter()
                .map(|frame| match &flag {
                    Some(value) => frame.slot(FLASH_LIMITER_SLOT, value.clone()),
                    None => frame,
                });
        let presented = rig.run(frames);
        assert!(worst(&presented, fps, FULL) <= 6, "{flag:?}");
    }
}

#[test]
fn capture_bytes_ignore_the_limiter_and_leave_its_history_alone() {
    let fps = 60.0;
    let strobe_frames = || strobe(FULL, fps, 0.6, |t| square(t, 5.0));
    let still = scene(FULL, gray(0.6));

    let mut captured = rig_or_skip!();
    let mut untouched = rig_or_skip!();
    captured.run(strobe_frames());
    untouched.run(strobe_frames());
    let on = captured.capture(&still);

    let mut off = rig_or_skip!();
    off.run(strobe_frames().into_iter().map(Frame::limiter_off));
    assert_eq!(
        on,
        off.capture(&still),
        "capture bytes do not depend on the limiter"
    );

    // The rig that captured presents exactly what the one that did not does.
    let a = captured.run(strobe_frames());
    let b = untouched.run(strobe_frames());
    for (a, b) in a.iter().zip(&b) {
        assert_eq!(a.bytes, b.bytes);
    }
}

#[test]
fn an_equal_brightness_red_green_flicker_is_held_and_presents_desaturated() {
    let fps = 60.0;
    let red = [1.0, 0.0, 0.0];
    let green = [0.0, 0.2126 / 0.7152, 0.0];
    let mut rig = rig_or_skip!();
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| {
            let t = n as f32 / fps;
            let rgb = if square(t, 5.0) > 0.5 { red } else { green };
            Frame::scene(scene(FULL, rgb), 1.0 / fps)
        })
        .collect();
    let presented = rig.run(frames);
    let mut counter = RedTransitionCounter::default();
    for (n, p) in presented.iter().enumerate() {
        counter.push(n as f32 / fps, p.mean_rgb(FULL));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(worst <= 6, "{worst} red transitions in one second");
    // Over budget, a frame whose content is red presents without its hue.
    let desaturated = presented.iter().enumerate().any(|(n, p)| {
        let content_red = square(n as f32 / fps, 5.0) > 0.5;
        let [r, g, b] = p.mean_rgb(FULL);
        content_red && r + g + b > 0.1 && r / (r + g + b) < 0.8
    });
    assert!(
        desaturated,
        "over budget, the red state presents desaturated"
    );
}

#[test]
fn a_single_red_transition_keeps_its_hue() {
    let mut rig = rig_or_skip!();
    rig.present(&Frame::scene(scene(FULL, gray(0.0)), 1.0 / 60.0));
    let mut last = None;
    for _ in 0..30 {
        last = Some(rig.present(&Frame::scene(scene(FULL, [1.0, 0.0, 0.0]), 1.0 / 60.0)));
    }
    let [r, g, b] = last.unwrap().mean_rgb(FULL);
    assert!(r > 0.9 && g < 0.01 && b < 0.01, "{r} {g} {b}");
}

/// Gameplay → splash → gameplay load cycles: bright gameplay, then the first
/// resolve frame after a 0.05 s splash stretch. Returns each first frame's
/// luminance.
fn load_cycles(rig: &mut Rig, cycles: usize) -> Vec<f32> {
    let mut first_frames = Vec::new();
    for _ in 0..cycles {
        for _ in 0..3 {
            rig.present(&Frame::scene(scene(FULL, gray(1.0)), 1.0 / 60.0));
        }
        let first = rig.present(&Frame::scene(scene(FULL, gray(1.0)), 0.07).after_splash(0.05));
        first_frames.push(first.luminance(FULL));
    }
    first_frames
}

#[test]
fn each_load_counts_both_splash_edges_and_rests_at_the_splash_over_budget() {
    // L14 and hub AC 5: each cycle is two transitions, the drop into the
    // splash and the return. Two cycles spend four; the third drop is the
    // fifth, and the drop that follows any further return cannot be
    // suppressed, so the third return rests at the splash level. Were a cycle
    // one transition, the third return would still fit.
    let mut rig = rig_or_skip!();
    let firsts = load_cycles(&mut rig, 3);
    assert!(firsts[..2].iter().all(|&l| l > 0.1), "{firsts:?}");
    assert!(
        firsts[2] < 0.05,
        "over budget, the load rests at the splash: {firsts:?}"
    );
    // The first resolve frame after each stretch is limited against the
    // splash: it rises from the splash at the capped rate, not to full.
    assert!(firsts[0] < 0.2, "{firsts:?}");
}

#[test]
fn a_quarter_screen_scene_strobe_is_held_like_a_full_screen_one() {
    let fps = 60.0;
    let mut rig = rig_or_skip!();
    let presented = rig.run(strobe(quarter(), fps, 2.0, |t| square(t, 6.0)));
    assert!(worst(&presented, fps, quarter()) <= 6);
}

#[test]
fn below_threshold_strobe_straddling_cells_passes_unchanged() {
    // 38×38 of 160×90 is 0.100 of the frame, below the 0.111 threshold, and
    // straddles cell boundaries (10-pixel cells) on all four sides, touching
    // 25 cells.
    let rect = Rect {
        x: 5,
        y: 5,
        w: 38,
        h: 38,
    };
    let mut limited = rig_or_skip!();
    let mut unlimited = rig_or_skip!();
    let on = limited.run(strobe(rect, 60.0, 2.0, |t| square(t, 8.0)));
    let off = unlimited.run(
        strobe(rect, 60.0, 2.0, |t| square(t, 8.0))
            .into_iter()
            .map(Frame::limiter_off),
    );
    for (n, (a, b)) in on.iter().zip(&off).enumerate() {
        assert!(
            a.bytes == b.bytes,
            "frame {n}: below-threshold strobe was altered"
        );
    }
}

#[test]
fn threshold_area_strobe_straddling_cells_is_limited() {
    // 40×40 of 160×90 is 0.111 of the frame — the threshold — straddling the
    // same boundaries as the passing case.
    let rect = Rect {
        x: 5,
        y: 5,
        w: 40,
        h: 40,
    };
    let mut rig = rig_or_skip!();
    let fps = 60.0;
    let presented = rig.run(strobe(rect, fps, 3.0, |t| square(t, 8.0)));
    assert!(worst(&presented, fps, rect) <= 6);
}

#[test]
fn an_uneven_flash_counts_its_whole_area() {
    // Two cells jump to white while the thirty around and beside them rise to
    // 0.25: 32 cells, 22% of the frame, strobing at 10 Hz. Weighed against the
    // brightest change, each dim cell read as a quarter and the whole as 9.5
    // cells, under the threshold, so the strobe passed unlimited. Each cell's
    // own coverage counts all 32.
    let ring = cells(4, 2, 6, 5);
    let beside = cells(10, 2, 1, 2);
    let core = cells(6, 4, 2, 1);
    let fps = 60.0;
    let dark = scene(FULL, gray(0.0));
    let lit = scene_of(&[(ring, gray(0.25)), (beside, gray(0.25)), (core, gray(1.0))]);
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| {
            let on = square(n as f32 / fps, 10.0) > 0.5;
            Frame::scene(if on { lit.clone() } else { dark.clone() }, 1.0 / fps)
        })
        .collect();
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let worst = worst(&presented, fps, ring);
    assert!(worst <= 6, "{worst} transitions in one second");
}

#[test]
fn a_still_cell_holding_an_old_excursion_does_not_shrink_a_later_flash() {
    // Two cells brighten to white once, below the flash area, and hold still;
    // their excursion stays uncounted. A 20-cell strobe at 0.25 then starts
    // beside them. Weighed against the still cells' old change, it read as
    // about four cells and passed unlimited.
    let still = cells(7, 5, 1, 2);
    let strobe_rect = cells(8, 4, 5, 4);
    let fps = 60.0;
    let dark = scene(FULL, gray(0.0));
    let held = scene(still, gray(1.0));
    let lit = scene_of(&[(still, gray(1.0)), (strobe_rect, gray(0.25))]);
    let start = 30;
    let frames: Vec<Frame> = (0..(3.0 * fps) as usize)
        .map(|n| {
            let pixels = if n < 5 {
                dark.clone()
            } else if n < start || square((n - start) as f32 / fps, 10.0) < 0.5 {
                held.clone()
            } else {
                lit.clone()
            };
            Frame::scene(pixels, 1.0 / fps)
        })
        .collect();
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let worst = worst(&presented[start..], fps, strobe_rect);
    assert!(worst <= 6, "{worst} transitions in one second");
}

#[test]
fn an_over_budget_red_onset_holds_exactly_its_last_redness() {
    // Red against a reddish partner of equal luminance at 5 Hz. Mixing toward
    // gray in proportion to the redness to shed overshoots, since redness is
    // not linear in the mix; storing that presented redness let each held
    // frame creep redder, back to saturated red within a few frames.
    let fps = 60.0;
    let red = [1.0, 0.0, 0.0];
    let partner = [0.2535, 0.2218, 0.0];
    let is_red = |n: usize| square(n as f32 / fps, 5.0) > 0.5;
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| {
            Frame::scene(
                scene(FULL, if is_red(n) { red } else { partner }),
                1.0 / fps,
            )
        })
        .collect();
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);

    let mut counter = RedTransitionCounter::default();
    for (n, p) in presented.iter().enumerate() {
        counter.push(n as f32 / fps, p.mean_rgb(FULL));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(worst <= 6, "{worst} red transitions in one second");

    // Redness on the frame limiter's scale: 0 neutral, 1 pure red.
    let redness = |[r, g, b]: [f32; 3]| ((r / (r + g + b) - 1.0 / 3.0) * 1.5).clamp(0.0, 1.0);
    let held = redness(presented[0].mean_rgb(FULL));
    let mut held_frames = 0;
    for (n, p) in presented.iter().enumerate() {
        if !is_red(n) {
            continue;
        }
        let shown = redness(p.mean_rgb(FULL));
        if shown < 0.95 {
            held_frames += 1;
            assert!(
                shown <= held + 0.03,
                "frame {n}: held redness crept from {held} to {shown}"
            );
        }
    }
    assert!(held_frames > 0, "over budget, red onsets are held");
}

#[test]
fn a_luminance_and_a_red_onset_in_one_frame_cannot_share_the_last_room() {
    // Two white flashes on the left spend four transitions. Then, in one
    // frame, a third white onset counts on the left and the right turns
    // saturated red at its own luminance. Only one onset and its return fit;
    // admitting both took the window to seven, then eight.
    let fps = 60.0;
    let dt = 1.0 / fps;
    let left = Rect {
        x: 0,
        y: 0,
        w: WIDTH / 2,
        h: HEIGHT,
    };
    let right = Rect {
        x: WIDTH / 2,
        y: 0,
        w: WIDTH / 2,
        h: HEIGHT,
    };
    let lead = 5;
    let red_from = lead + 30;
    // Stops before the first transition ages out, so the held red onset is
    // never admitted inside the run.
    let frames: Vec<Frame> = (0..lead + 61)
        .map(|n| {
            let k = n as i64 - lead as i64;
            // The rate cap counts a white onset on its second frame, so the
            // third starts one frame before the red.
            let white = (0..6).contains(&k) || (15..21).contains(&k) || (29..36).contains(&k);
            let right_rgb = if n >= red_from {
                [1.0, 0.0, 0.0]
            } else {
                gray(0.2126)
            };
            let pixels = scene_of(&[
                (left, gray(if white { 1.0 } else { 0.0 })),
                (right, right_rgb),
            ]);
            Frame::scene(pixels, dt)
        })
        .collect();
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);

    let mut times = Vec::new();
    for rect in [left, right] {
        let mut luminance = TransitionCounter::default();
        let mut red = RedTransitionCounter::default();
        for (n, p) in presented.iter().enumerate() {
            luminance.push(n as f32 * dt, p.luminance(rect));
            red.push(n as f32 * dt, p.mean_rgb(rect));
        }
        times.extend_from_slice(luminance.transition_times());
        times.extend_from_slice(red.transition_times());
    }
    times.sort_by(f32::total_cmp);
    let worst = max_transitions_in_any_second(&times);
    assert!(worst <= 6, "{worst} transitions in one second: {times:?}");
    // The red onset waits: the right presents without its hue.
    let [r, g, b] = presented[red_from].mean_rgb(right);
    assert!(r / (r + g + b) < 0.8, "{r} {g} {b}");
}

#[test]
fn a_strobe_from_a_load_after_a_fade_to_black_is_held_to_three_flashes() {
    // The last gameplay frame is darker than the splash, so the edge into the
    // splash is a small brightening that does not count. The first brightening
    // after the load continues it and must still count; marking the drop
    // counted let it pass uncounted, and the first second showed seven
    // transitions.
    let fps = 60.0;
    let dt = 1.0 / fps;
    let splash_elapsed = 0.07;
    let mut frames = Vec::new();
    // Bright gameplay, then a fade to black the limiter counts, held long
    // enough to present black.
    for _ in 0..4 {
        frames.push(Frame::scene(scene(FULL, gray(1.0)), dt));
    }
    for _ in 0..24 {
        frames.push(Frame::scene(scene(FULL, gray(0.0)), dt));
    }
    // A load: a short splash, then a strobe that starts bright.
    let load = frames.len();
    for n in 0..60 {
        let frame = Frame::scene(scene(FULL, gray(1.0 - square(n as f32 * dt, 5.0))), dt);
        frames.push(if n == 0 {
            Frame {
                elapsed: splash_elapsed,
                ..frame
            }
            .after_splash(0.05)
        } else {
            frame
        });
    }
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    assert!(
        presented[load - 1].luminance(FULL) < 0.005,
        "gameplay ends darker than the splash"
    );

    let mut counter = TransitionCounter::default();
    let mut t = 0.0;
    for (n, p) in presented.iter().enumerate() {
        t += if n == load { splash_elapsed } else { dt };
        counter.push(t, p.luminance(FULL));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(worst <= 6, "{worst} transitions in one second");
}

#[test]
fn over_budget_a_sub_threshold_brightening_passes_while_a_flash_is_still_held() {
    // Owner decision B: three full-screen flashes spend the budget. A slow
    // brightening that stays below the flash threshold then presents as
    // authored — it used to be held with every other brightening — while a
    // fourth flash is still held.
    let fps = 60.0;
    let dt = 1.0 / fps;
    let lead = 5;
    let level = |n: usize| -> f32 {
        let Some(k) = n.checked_sub(lead) else {
            return 0.0;
        };
        if k < 45 {
            // Three flashes, six frames on and nine off.
            return if k % 15 < 6 { 1.0 } else { 0.0 };
        }
        match k {
            // +0.02 a frame, to 0.08.
            47..=50 => 0.02 * (k - 46) as f32,
            51..=56 => 1.0,
            _ => 0.0,
        }
    };
    let mut rig = rig_or_skip!();
    let presented = rig.run((0..lead + 62).map(|n| Frame::scene(scene(FULL, gray(level(n))), dt)));
    for (n, p) in presented.iter().enumerate().take(lead + 51).skip(lead + 47) {
        let shown = p.luminance(FULL);
        assert!(
            (shown - level(n)).abs() < 0.004,
            "frame {n}: a sub-threshold brightening was held at {shown}, authored {}",
            level(n)
        );
    }
    for (n, p) in presented.iter().enumerate().take(lead + 57).skip(lead + 51) {
        let shown = p.luminance(FULL);
        assert!(
            shown < 0.09,
            "frame {n}: the fourth flash presented {shown}"
        );
    }
    assert!(worst(&presented, fps, FULL) <= 6);
}

/// Five dark frames, then three full-screen flashes (six frames on, nine off)
/// that spend the whole budget, ending dark.
fn spend_budget(fps: f32) -> Vec<Frame> {
    (0..50)
        .map(|n| {
            let lit = n >= 5 && (n - 5) % 15 < 6;
            Frame::scene(scene(FULL, gray(if lit { 1.0 } else { 0.0 })), 1.0 / fps)
        })
        .collect()
}

/// Worst transitions in any one second of `level` over `presented`.
fn worst_of(presented: &[Presented], fps: f32, level: impl Fn(&Presented) -> f32) -> usize {
    let mut counter = TransitionCounter::default();
    for (n, frame) in presented.iter().enumerate() {
        counter.push(n as f32 / fps, level(frame));
    }
    max_transitions_in_any_second(counter.transition_times())
}

#[test]
fn over_budget_a_flash_that_lights_in_two_frames_is_held() {
    // Three full-screen flashes spend the budget. A 20-cell flash (13.9% of
    // the frame) then strobes at 5 Hz, its left half lighting a frame before
    // its right and going dark a frame before it. The left alone is under the
    // flash area and passes. The right completes the flash while the left
    // holds still, its rise uncounted; weighing only cells rising that frame,
    // the hold saw 10 cells and let the right through, and the whole flash
    // showed about ten transitions a second.
    let fps = 60.0;
    let left = cells(0, 0, 5, 2);
    let right = cells(5, 0, 5, 2);
    let mut frames = spend_budget(fps);
    let start = frames.len();
    frames.extend((0..(2.0 * fps) as usize).map(|k| {
        let phase = k % 12;
        let lit = |on: bool| gray(if on { 1.0 } else { 0.0 });
        Frame::scene(
            scene_of(&[
                (left, lit(phase <= 5)),
                (right, lit((1..=6).contains(&phase))),
            ]),
            1.0 / fps,
        )
    }));
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let after = &presented[start..];
    let right_alone = worst(after, fps, right);
    // Both halves lit together: the 20-cell flash itself.
    let together = worst_of(after, fps, |p| p.luminance(left).min(p.luminance(right)));
    assert!(
        right_alone <= 6,
        "{right_alone} transitions of the right half"
    );
    assert!(together <= 6, "{together} transitions of the whole flash");
}

#[test]
fn over_budget_a_red_flash_that_saturates_in_two_frames_stays_held() {
    // The red counterpart, at equal luminance against gray: three full-screen
    // red flashes spend the budget, then the right half turns red a frame
    // after the left and back a frame after it. The held right half must stay
    // desaturated when the left returns first, not show one red frame.
    let fps = 60.0;
    let background = gray(0.2126);
    let red = [1.0, 0.0, 0.0];
    let left = cells(0, 0, 5, 2);
    let right = cells(5, 0, 5, 2);
    let mut frames: Vec<Frame> = (0..50)
        .map(|n| {
            let lit = n >= 5 && (n - 5) % 15 < 6;
            Frame::scene(scene(FULL, if lit { red } else { background }), 1.0 / fps)
        })
        .collect();
    let start = frames.len();
    frames.extend((0..(2.0 * fps) as usize).map(|k| {
        let phase = k % 12;
        let tint = |on: bool| if on { red } else { background };
        Frame::scene(
            scene_of(&[
                (FULL, background),
                (left, tint(phase <= 5)),
                (right, tint((1..=6).contains(&phase))),
            ]),
            1.0 / fps,
        )
    }));
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let mut counter = RedTransitionCounter::default();
    for (n, p) in presented[start..].iter().enumerate() {
        counter.push(n as f32 / fps, p.mean_rgb(right));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(worst <= 6, "{worst} red transitions of the right half");
}

#[test]
fn over_budget_a_held_band_holds_its_partly_covered_edges() {
    // Three full-screen flashes spend the budget. A full-width band at 0.14
    // then strobes at 5 Hz: row 4 fully covered, rows 3 and 5 70% covered
    // (cell means 0.098). Rate-capped, every row's candidate change was the
    // same, so the edge rows read as fully covered and their 0.098 as under
    // the threshold: they flashed unheld, 22 cells of 0 ↔ 0.14 pixels.
    let fps = 60.0;
    let band = Rect {
        x: 0,
        y: 33,
        w: WIDTH,
        h: 24,
    };
    let edges = [
        Rect {
            x: 0,
            y: 33,
            w: WIDTH,
            h: 7,
        },
        Rect {
            x: 0,
            y: 50,
            w: WIDTH,
            h: 7,
        },
    ];
    let mut frames = spend_budget(fps);
    let start = frames.len();
    frames.extend(strobe(band, fps, 2.0, |t| 0.14 * square(t, 5.0)));
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    for edge in edges {
        let edge_worst = worst(&presented[start..], fps, edge);
        assert!(edge_worst <= 6, "{edge_worst} transitions in {edge:?}");
    }
}

#[test]
fn a_bands_partly_covered_rows_count_at_their_cover() {
    // In budget, rows 3 and 5 of a band at y 33..57 are 70% covered. Eight
    // cells wide it covers 8 + 2 × 8 × 0.7 = 19.2 cells and is limited; six
    // wide, 14.4 cells, it passes unchanged.
    let fps = 60.0;
    let band = |cells_wide: u32| Rect {
        x: 0,
        y: 33,
        w: cells_wide * 10,
        h: 24,
    };
    let level = |t: f32| 0.14 * square(t, 5.0);
    let mut rig = rig_or_skip!();
    let presented = rig.run(strobe(band(8), fps, 2.0, level));
    let wide = worst(&presented, fps, band(8));
    assert!(wide <= 6, "{wide} transitions of the 19.2-cell band");

    let mut limited = rig_or_skip!();
    let mut unlimited = rig_or_skip!();
    let on = limited.run(strobe(band(6), fps, 2.0, level));
    let off = unlimited.run(
        strobe(band(6), fps, 2.0, level)
            .into_iter()
            .map(Frame::limiter_off),
    );
    for (n, (a, b)) in on.iter().zip(&off).enumerate() {
        assert!(
            a.bytes == b.bytes,
            "frame {n}: the 14.4-cell band was altered"
        );
    }
}

/// A red square at (5, 5) flickering against an equal-luminance gray at
/// 5 Hz, straddling 10-pixel cells on all four sides.
fn red_square_over_gray(side: u32, fps: f32) -> (Rect, Vec<Frame>) {
    let rect = Rect {
        x: 5,
        y: 5,
        w: side,
        h: side,
    };
    let background = gray(0.2126);
    let frames = (0..(2.0 * fps) as usize)
        .map(|n| {
            let on = square(n as f32 / fps, 5.0) > 0.5;
            let pixels = if on {
                scene_of(&[(FULL, background), (rect, [1.0, 0.0, 0.0])])
            } else {
                scene(FULL, background)
            };
            Frame::scene(pixels, 1.0 / fps)
        })
        .collect();
    (rect, frames)
}

#[test]
fn a_threshold_area_red_strobe_straddling_cells_over_gray_is_limited() {
    // 44×44 is 19.4 cells of area. Judged on cell means, a partly covered
    // cell over gray never reached saturated red (a half-covered cell reads
    // 0.61), so only 15.5 cells counted and the strobe passed unlimited.
    let fps = 60.0;
    let (rect, frames) = red_square_over_gray(44, fps);
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let mut counter = RedTransitionCounter::default();
    for (n, p) in presented.iter().enumerate() {
        counter.push(n as f32 / fps, p.mean_rgb(rect));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(worst <= 6, "{worst} red transitions in one second");
}

#[test]
fn a_below_threshold_red_strobe_straddling_cells_over_gray_passes_unchanged() {
    // 38×38, 14.4 cells of area: its partly covered cells count at their
    // cover, so it stays under the threshold.
    let fps = 60.0;
    let (_, frames) = red_square_over_gray(38, fps);
    let (_, unlimited_frames) = red_square_over_gray(38, fps);
    let mut limited = rig_or_skip!();
    let mut unlimited = rig_or_skip!();
    let on = limited.run(frames);
    let off = unlimited.run(unlimited_frames.into_iter().map(Frame::limiter_off));
    for (n, (a, b)) in on.iter().zip(&off).enumerate() {
        assert!(
            a.bytes == b.bytes,
            "frame {n}: below-threshold red strobe was altered"
        );
    }
}

#[test]
fn a_strobe_that_dips_slightly_on_the_way_is_still_held_to_three_flashes() {
    // 0 ↔ 0.19 at 6 Hz, climbing in 0.065 steps (under the rate cap) with a
    // 0.001 dip between them. Each dip restarted the excursion, so no 0.065
    // step ever reached the threshold, and twelve transitions a second passed.
    let fps = 60.0;
    let cycle = [
        0.0, 0.065, 0.064, 0.129, 0.128, 0.193, 0.128, 0.129, 0.064, 0.065,
    ];
    let frames: Vec<Frame> = (0..(2.0 * fps) as usize)
        .map(|n| Frame::scene(scene(FULL, gray(cycle[n % cycle.len()])), 1.0 / fps))
        .collect();
    let mut rig = rig_or_skip!();
    let presented = rig.run(frames);
    let worst = worst(&presented, fps, FULL);
    assert!(worst <= 6, "{worst} transitions in one second");
}

#[test]
fn a_slow_strobe_whose_reversals_reach_the_threshold_is_counted() {
    // A triangle between 0.35 and 0.47 at 5 Hz, well under the rate cap. Each
    // 0.12 reversal reaches the threshold, so it ends the excursion and
    // counts: the strobe is limited, and still shows its first flashes.
    let fps = 60.0;
    let presented = {
        let mut rig = rig_or_skip!();
        rig.run(strobe(FULL, fps, 2.0, |t| {
            let phase = (t * 5.0).fract() * 2.0;
            0.35 + 0.12 * if phase <= 1.0 { phase } else { 2.0 - phase }
        }))
    };
    let worst = worst(&presented, fps, FULL);
    assert!(
        (4..=6).contains(&worst),
        "{worst} transitions in one second"
    );
}

/// Wall-clock cost of the resolve with the frame limiter on, off (the measure
/// pass skipped, the limit pass writing identity) and bypassed (the test-only
/// switch, which also skips the measure pass), at 1920×1080 and 3840×2160. A
/// measurement, not a gate: each mode encodes and submits `FRAMES` resolves
/// one command buffer per frame, then waits for the GPU; the best of several
/// rounds is printed as ms per frame. Wall-clock time includes submission
/// overhead, so it bounds the GPU cost from above; per-pass GPU time needs a
/// timestamp-capable adapter (`POSTRETRO_GPU_TIMING=1`).
///
/// Run: `cargo test -p postretro-renderer --lib flash_limiter_resolve_cost --
/// --ignored --nocapture` (a release build gives steadier numbers: add
/// `--release`).
#[test]
#[ignore = "measurement, not a gate; run with --ignored --nocapture"]
fn flash_limiter_resolve_cost() {
    const FRAMES: u32 = 60;
    const ROUNDS: u32 = 5;
    let Some(ctx) = try_init_gpu() else {
        eprintln!("flash limiter cost: no adapter, skipping (not a pass)");
        return;
    };
    for [width, height] in [[1920u32, 1080u32], [3840, 2160]] {
        let mut pass = ScreenEffectsPass::new(&ctx.device, width, height, PRESENT_FORMAT);
        // A varied HDR scene so the measure pass samples real content.
        let one = f32_to_f16_bits(1.0);
        let mut data = Vec::with_capacity((width * height * 8) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = f32_to_f16_bits(x as f32 / width as f32);
                let g = f32_to_f16_bits(y as f32 / height as f32);
                let b = f32_to_f16_bits(((x ^ y) & 0xff) as f32 / 255.0);
                for half in [r, g, b, one] {
                    data.extend_from_slice(&half.to_le_bytes());
                }
            }
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pass.scene_color_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 8),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Flash limiter cost target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PRESENT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let on = HashMap::new();
        let off = HashMap::from([(FLASH_LIMITER_SLOT.to_string(), SlotValue::Boolean(false))]);

        let run = |pass: &mut ScreenEffectsPass, slots: &HashMap<String, SlotValue>| {
            let start = std::time::Instant::now();
            for _ in 0..FRAMES {
                let mut encoder = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                pass.encode_resolve(
                    &ctx.queue,
                    &mut encoder,
                    &view,
                    slots,
                    LimiterFrameInput::default(),
                    ResolveTimestamps::default(),
                );
                ctx.queue.submit(std::iter::once(encoder.finish()));
            }
            let recorded = start.elapsed().as_secs_f64();
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            let per_frame = |seconds: f64| seconds * 1000.0 / f64::from(FRAMES);
            [
                per_frame(start.elapsed().as_secs_f64()),
                per_frame(recorded),
            ]
        };

        // Warm up pipelines and the upload.
        run(&mut pass, &on);
        // Per mode: best total (to the GPU finishing) and best CPU recording.
        let mut best = [[f64::MAX; 2]; 3];
        let keep = |slot: &mut [f64; 2], sample: [f64; 2]| {
            slot[0] = slot[0].min(sample[0]);
            slot[1] = slot[1].min(sample[1]);
        };
        for _ in 0..ROUNDS {
            pass.bypass_frame_limiter = false;
            keep(&mut best[0], run(&mut pass, &on));
            keep(&mut best[1], run(&mut pass, &off));
            pass.bypass_frame_limiter = true;
            keep(&mut best[2], run(&mut pass, &on));
        }
        println!(
            "flash limiter resolve cost {width}x{height} (ms/frame, total / CPU recording): \
             limiter on {:.3} / {:.3}, limiter off {:.3} / {:.3}, bypassed {:.3} / {:.3}; \
             on - off = {:.3}",
            best[0][0],
            best[0][1],
            best[1][0],
            best[1][1],
            best[2][0],
            best[2][1],
            best[0][0] - best[1][0],
        );
    }
}

#[path = "flash_limiter_motion_test.rs"]
mod motion;
