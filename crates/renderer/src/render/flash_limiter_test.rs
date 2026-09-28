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
use super::screen_effects::ScreenEffectsPass;
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

/// One synthetic `scene_color` frame: a black field with `rect` at `rgb`.
fn scene(rect: Rect, rgb: [f32; 3]) -> Vec<u8> {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 8) as usize);
    let lit = rgb.map(f32_to_f16_bits);
    let one = f32_to_f16_bits(1.0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
            let px = if inside { lit } else { [0; 3] };
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
            None,
            None,
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
fn turning_the_limiter_on_mid_strobe_limits_from_that_frame_against_the_last_presented() {
    // UO5 and hub AC 4: on for 0.5 s, off 0.5 s, then on again.
    let fps = 60.0;
    let mut rig = rig_or_skip!();
    let frames = strobe(FULL, fps, 3.0, |t| square(t, 5.0));
    let mut presented = Vec::new();
    let mut reenabled_at = None;
    let mut first_reenabled_matches_input = false;
    for (n, frame) in frames.into_iter().enumerate() {
        let t = n as f32 / fps;
        // Re-enabled mid-way through a dark half-period, so the first frame
        // after matches the frame presented just before it.
        let off = (0.5..1.05).contains(&t);
        let input_level = if frame.pixels[0] == 0 && frame.pixels[1] == 0 {
            0.0
        } else {
            1.0
        };
        let reenables = !off && t >= 1.05 && reenabled_at.is_none();
        let p = rig.present(&if off { frame.limiter_off() } else { frame });
        if reenables {
            reenabled_at = Some(n);
            first_reenabled_matches_input = (p.luminance(FULL) - input_level).abs() < 0.05;
        }
        presented.push(p);
    }
    let start = reenabled_at.unwrap();
    // The first frame after re-enabling matches the frame presented just
    // before it (a strobe half-period is several frames), so it presents
    // unchanged.
    assert!((presented[start].luminance(FULL) - presented[start - 1].luminance(FULL)).abs() < 0.05);
    assert!(first_reenabled_matches_input);
    // No transition from before the off counts: the window after re-enabling
    // admits a full three flashes, and never more.
    let after = worst(&presented[start..], fps, FULL);
    assert!((4..=6).contains(&after), "{after}");
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
