// GPU tests for the photosensitivity flash limiter: synthetic `scene_color`
// sequences through the measure pass and the resolve into an offscreen target,
// read back every frame and counted by an independent WCAG flash counter.
// A run with no adapter skips; a skip is not a pass.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use std::collections::HashMap;

use postretro_entities::SlotValue;
use postretro_render_cpu::flash_limiter::{FLASH_LIMITER_SLOT, LimiterFrameInput};

use super::gpu_test_harness::{GpuCtx, read_texture_rgba8, try_init_gpu};
use super::screen_effects::ScreenEffectsPass;
use super::wcag_flash_counter::{
    TransitionCounter, max_transitions_in_any_second, srgb8_to_linear,
};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const PRESENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A rectangle of the synthetic scene, in pixels.
#[derive(Clone, Copy)]
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

/// One synthetic `scene_color` frame: a black field with `rect` at `level`.
fn scene_pixels(rect: Rect, level: f32) -> Vec<u8> {
    let mut data = Vec::with_capacity((WIDTH * HEIGHT * 8) as usize);
    let lit = f32_to_f16_bits(level);
    let one = f32_to_f16_bits(1.0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
            let v = if inside { lit } else { 0 };
            for half in [v, v, v, one] {
                data.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    data
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

struct Rig {
    ctx: GpuCtx,
    pass: ScreenEffectsPass,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
}

impl Rig {
    fn new() -> Option<Self> {
        let ctx = try_init_gpu()?;
        let pass = ScreenEffectsPass::new(&ctx.device, WIDTH, HEIGHT, PRESENT_FORMAT);
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Flash limiter test target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PRESENT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        Some(Self {
            ctx,
            pass,
            target,
            target_view,
        })
    }

    /// Present one frame of `pixels` and return the read-back RGBA8 bytes.
    fn present(
        &mut self,
        pixels: &[u8],
        slots: &HashMap<String, SlotValue>,
        elapsed_seconds: f32,
    ) -> Vec<u8> {
        self.ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.pass.scene_color_texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 8),
                rows_per_image: Some(HEIGHT),
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.pass.encode_resolve(
            &self.ctx.queue,
            &mut encoder,
            &self.target_view,
            slots,
            LimiterFrameInput { elapsed_seconds },
        );
        read_texture_rgba8(&self.ctx, &self.target, WIDTH, HEIGHT, encoder).pixels
    }
}

fn limiter_off() -> HashMap<String, SlotValue> {
    HashMap::from([(FLASH_LIMITER_SLOT.to_string(), SlotValue::Boolean(false))])
}

/// Mean relative luminance of `rect` in read-back sRGB8 bytes.
fn region_luminance(bytes: &[u8], rect: Rect) -> f32 {
    let mut sum = 0.0;
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            let i = ((y * WIDTH + x) * 4) as usize;
            let [r, g, b] = [bytes[i], bytes[i + 1], bytes[i + 2]].map(srgb8_to_linear);
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    sum / (rect.w * rect.h) as f32
}

/// Square-wave strobe level at time `t`: dark for the first half of each
/// period, so every flash is a brightening from a dark rest.
fn square(t: f32, hz: f32) -> f32 {
    if (t * hz).fract() < 0.5 { 0.0 } else { 1.0 }
}

/// Run a square strobe of `rect` for `seconds` at `fps`, returning each
/// presented frame's bytes.
fn run_square_strobe(
    rig: &mut Rig,
    rect: Rect,
    hz: f32,
    fps: f32,
    seconds: f32,
    slots: &HashMap<String, SlotValue>,
) -> Vec<Vec<u8>> {
    let dt = 1.0 / fps;
    let frames = (seconds * fps).round() as u32;
    (0..frames)
        .map(|n| {
            let level = square(n as f32 * dt, hz);
            rig.present(&scene_pixels(rect, level), slots, dt)
        })
        .collect()
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

#[test]
fn full_screen_square_strobe_is_held_to_three_flashes_per_second() {
    let mut rig = rig_or_skip!();
    let fps = 60.0;
    let frames = run_square_strobe(&mut rig, FULL, 5.0, fps, 3.0, &HashMap::new());

    let mut counter = TransitionCounter::default();
    for (n, bytes) in frames.iter().enumerate() {
        counter.push(n as f32 / fps, region_luminance(bytes, FULL));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(
        worst <= 6,
        "presented {worst} transitions in one second; at most three flashes (six transitions) allowed"
    );
    // The unlimited strobe has ten flashes per second; the limiter must not
    // have dimmed it to nothing either.
    assert!(
        worst >= 4,
        "limited strobe should still show flashes, got {worst} transitions"
    );
    // Over budget, the strobe rests at its pre-flash (dark) level: somewhere in
    // the first second a whole flash period presents no brightening.
    let period_frames = (fps / 5.0) as usize;
    let rests_dark = frames[..fps as usize]
        .windows(period_frames)
        .any(|w| w.iter().all(|bytes| region_luminance(bytes, FULL) < 0.1));
    assert!(
        rests_dark,
        "an over-budget flash should be suppressed at its onset"
    );
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
    let on = run_square_strobe(&mut limited, rect, 8.0, 60.0, 2.0, &HashMap::new());
    let off = run_square_strobe(&mut unlimited, rect, 8.0, 60.0, 2.0, &limiter_off());
    for (n, (a, b)) in on.iter().zip(&off).enumerate() {
        assert!(a == b, "frame {n}: below-threshold strobe was altered");
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
    let frames = run_square_strobe(&mut rig, rect, 8.0, fps, 3.0, &HashMap::new());
    let mut counter = TransitionCounter::default();
    for (n, bytes) in frames.iter().enumerate() {
        counter.push(n as f32 / fps, region_luminance(bytes, rect));
    }
    let worst = max_transitions_in_any_second(counter.transition_times());
    assert!(
        worst <= 6,
        "threshold-area strobe presented {worst} transitions in one second"
    );
}
