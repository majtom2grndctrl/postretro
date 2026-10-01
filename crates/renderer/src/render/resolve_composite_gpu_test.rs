// GPU-harness tests for the resolve: nearest integer upscale, the UI-layer
// composite, and the covers-HUD switches. Self-skip without an adapter.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashMap;

use postretro_entities::SlotValue;
use postretro_render_cpu::flash_limiter::{FLASH_LIMITER_SLOT, LimiterFrameInput};
use postretro_render_cpu::render_extent::{Extent, scene_extent};
use postretro_render_cpu::screen_effects::CoversHud;

use super::gpu_test_harness::{GpuCtx, Readback, read_texture_rgba8, try_init_gpu};
use super::screen_effects::ScreenEffectsPass;

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// f16 bit patterns for exactly representable linear values.
const F16_ZERO: u16 = 0x0000;
const F16_ONE: u16 = 0x3C00;
const F16_LEVELS: [u16; 5] = [0x0000, 0x3000, 0x3400, 0x3800, 0x3A00]; // 0, .125, .25, .5, .75

fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

fn assert_rgb_near(actual: [u8; 4], expected: [u8; 3], tolerance: u8, what: &str) {
    for channel in 0..3 {
        assert!(
            actual[channel].abs_diff(expected[channel]) <= tolerance,
            "{what}: got {actual:?}, expected rgb {expected:?} (±{tolerance})"
        );
    }
}

struct Fixture {
    ctx: GpuCtx,
    pass: ScreenEffectsPass,
    surface: Extent,
    divisor: u32,
}

impl Fixture {
    fn new(ctx: GpuCtx, surface: Extent, divisor: u32) -> Self {
        let scene = scene_extent(surface, divisor);
        let pass = ScreenEffectsPass::new(&ctx.device, scene, surface, TARGET_FORMAT);
        Self {
            ctx,
            pass,
            surface,
            divisor,
        }
    }

    fn scene(&self) -> Extent {
        scene_extent(self.surface, self.divisor)
    }

    /// Fill scene colour with one f16 RGBA texel per pixel.
    fn write_scene(&self, texel: impl Fn(u32, u32) -> [u16; 4]) {
        let scene = self.scene();
        let mut bytes = Vec::with_capacity((scene.width * scene.height * 8) as usize);
        for y in 0..scene.height {
            for x in 0..scene.width {
                for half in texel(x, y) {
                    bytes.extend_from_slice(&half.to_le_bytes());
                }
            }
        }
        self.write(self.pass.scene_color_texture(), &bytes, scene, 8);
    }

    /// Set premultiplied sRGB UI texels; every other layer texel stays clear.
    fn write_ui(&self, texels: &[((u32, u32), [u8; 4])]) {
        let mut bytes = vec![0u8; (self.surface.width * self.surface.height * 4) as usize];
        for &((x, y), rgba) in texels {
            let i = ((y * self.surface.width + x) * 4) as usize;
            bytes[i..i + 4].copy_from_slice(&rgba);
        }
        self.write(self.pass.ui_layer_texture(), &bytes, self.surface, 4);
    }

    fn write(&self, texture: &wgpu::Texture, bytes: &[u8], size: Extent, texel_bytes: u32) {
        self.ctx.queue.write_texture(
            texture.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * texel_bytes),
                rows_per_image: Some(size.height),
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
    }

    fn resolve(&mut self, slots: &HashMap<String, SlotValue>) -> Readback {
        let target = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Resolve Test Target"),
            size: wgpu::Extent3d {
                width: self.surface.width,
                height: self.surface.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.pass.encode_resolve(
            &self.ctx.queue,
            &mut encoder,
            &view,
            slots,
            LimiterFrameInput::default(),
            self.divisor,
            None,
        );
        read_texture_rgba8(
            &self.ctx,
            &target,
            self.surface.width,
            self.surface.height,
            encoder,
        )
    }
}

fn no_effects() -> HashMap<String, SlotValue> {
    HashMap::new()
}

/// Flash half-green, full-strength blue vignette, and a 0.1-screen shake, with
/// the limiter off so every frame packs its slots unchanged.
fn all_effects() -> HashMap<String, SlotValue> {
    HashMap::from([
        (
            "screen.flash".to_string(),
            SlotValue::Array(vec![0.0, 1.0, 0.0, 0.5]),
        ),
        (
            "screen.vignette".to_string(),
            SlotValue::Array(vec![0.0, 0.0, 1.0, 1.0]),
        ),
        (
            "screen.shake".to_string(),
            SlotValue::Array(vec![128.0, 72.0]),
        ),
        (FLASH_LIMITER_SLOT.to_string(), SlotValue::Boolean(false)),
    ])
}

/// Each scene texel's colour is unique to its coordinate.
fn coordinate_texel(x: u32, y: u32) -> [u16; 4] {
    [
        F16_LEVELS[x as usize % 5],
        F16_LEVELS[y as usize % 5],
        F16_ZERO,
        F16_ONE,
    ]
}

// AC: when the surface is not divisible by the divisor, every visible scene
// pixel covers exactly divisor × divisor surface pixels; only the overshoot at
// the frame edge is cropped.
#[test]
fn resolve_replicates_each_scene_pixel_into_a_divisor_square_and_crops_the_edge() {
    for (surface, divisor) in [
        (Extent::new(14, 8), 3),
        (Extent::new(9, 7), 2),
        (Extent::new(10, 6), 2),
    ] {
        let Some(ctx) = try_init_gpu() else {
            return;
        };
        let mut fixture = Fixture::new(ctx, surface, divisor);
        let scene = fixture.scene();
        fixture.write_scene(coordinate_texel);
        fixture.write_ui(&[]);
        let out = fixture.resolve(&no_effects());

        let mut block_colours = Vec::new();
        for by in 0..scene.height {
            for bx in 0..scene.width {
                let origin = out.at(bx * divisor, by * divisor);
                block_colours.push(origin);
                let x_end = ((bx + 1) * divisor).min(surface.width);
                let y_end = ((by + 1) * divisor).min(surface.height);
                for y in by * divisor..y_end {
                    for x in bx * divisor..x_end {
                        assert_eq!(
                            out.at(x, y),
                            origin,
                            "{surface:?}/{divisor}: pixel ({x},{y}) leaves scene pixel ({bx},{by})"
                        );
                    }
                }
            }
        }
        let distinct: std::collections::HashSet<_> = block_colours.iter().collect();
        assert_eq!(
            distinct.len(),
            block_colours.len(),
            "{surface:?}/{divisor}: each scene pixel appears exactly once"
        );
    }
}

// AC: an opaque UI pixel reaches the swapchain with the colour it was drawn
// with, untouched by the tonemap.
#[test]
fn opaque_ui_pixel_reaches_the_target_untonemapped() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let mut fixture = Fixture::new(ctx, Extent::new(8, 4), 2);
    // A 1.0 scene tonemaps to 0.99; the same white drawn as UI must not.
    fixture.write_scene(|_, _| [F16_ONE, F16_ONE, F16_ONE, F16_ONE]);
    let white = [255, 255, 255, 255];
    let drawn = [200, 100, 50, 255];
    fixture.write_ui(&[((1, 1), white), ((5, 2), drawn)]);
    let out = fixture.resolve(&no_effects());

    assert_eq!(
        out.at(1, 1),
        white,
        "white UI is not compressed by the knee"
    );
    assert_eq!(out.at(5, 2), drawn);
    assert!(
        out.at(3, 3)[0] < 255,
        "the scene beneath is tonemapped below the display ceiling"
    );
}

// AC: each covers-HUD switch defaults off. With all switches off, effect
// strength leaves opaque UI pixels unchanged and translucent UI pixels change
// only through the scene beneath them.
#[test]
fn effects_with_switches_off_leave_opaque_ui_unchanged_and_reach_translucent_ui_through_the_scene()
{
    const SURFACE: Extent = Extent::new(14, 8);
    let opaque = [200, 100, 50, 255];
    let centre = (7, 4);
    let corner = (0, 0);
    let translucent_at = (3, 4);
    // Premultiplied: linear red 0.25 at 50% coverage.
    let translucent = [linear_to_srgb(0.125), 0, 0, 128];

    let run = |ui: &[((u32, u32), [u8; 4])]| {
        let ctx = try_init_gpu()?;
        let mut fixture = Fixture::new(ctx, SURFACE, 2);
        fixture.write_scene(|_, _| [0x3400, 0x3400, 0x3400, F16_ONE]);
        fixture.write_ui(ui);
        Some(fixture.resolve(&all_effects()))
    };
    let Some(with_ui) = run(&[
        (centre, opaque),
        (corner, opaque),
        (translucent_at, translucent),
    ]) else {
        return;
    };
    let scene_only = run(&[]).expect("adapter present");

    assert_eq!(
        with_ui.at(centre.0, centre.1),
        opaque,
        "flash, vignette, shake spare the HUD"
    );
    assert_eq!(
        with_ui.at(corner.0, corner.1),
        opaque,
        "the vignette edge spares the HUD"
    );

    let beneath = scene_only.at(translucent_at.0, translucent_at.1);
    let alpha = 128.0 / 255.0;
    let expected: [u8; 3] = std::array::from_fn(|c| {
        linear_to_srgb(srgb_to_linear(beneath[c]) * (1.0 - alpha) + srgb_to_linear(translucent[c]))
    });
    assert_rgb_near(
        with_ui.at(translucent_at.0, translucent_at.1),
        expected,
        2,
        "translucent UI over the effected scene",
    );
}

// AC: with one switch on in a test, only that effect reaches the UI.
#[test]
fn one_covers_hud_switch_lets_only_its_effect_reach_the_ui() {
    const SURFACE: Extent = Extent::new(14, 8);
    let opaque = [200, 100, 50, 255];
    let centre = (7u32, 4u32);
    let corner = (0u32, 0u32);
    let lin = |c: usize| srgb_to_linear(opaque[c]);

    let run = |covers: CoversHud| {
        let ctx = try_init_gpu()?;
        let mut fixture = Fixture::new(ctx, SURFACE, 2);
        fixture.pass.set_covers_hud(covers);
        fixture.write_scene(|_, _| [0x3400, 0x3400, 0x3400, F16_ONE]);
        fixture.write_ui(&[(centre, opaque), (corner, opaque)]);
        Some(fixture.resolve(&all_effects()))
    };

    // Flash: mix toward green by 0.5, everywhere on the UI.
    let Some(flash) = run(CoversHud {
        flash: true,
        ..CoversHud::default()
    }) else {
        return;
    };
    let flashed: [u8; 3] = std::array::from_fn(|c| {
        let target = if c == 1 { 1.0 } else { 0.0 };
        linear_to_srgb(lin(c) * 0.5 + target * 0.5)
    });
    assert_rgb_near(
        flash.at(centre.0, centre.1),
        flashed,
        1,
        "flash covers the HUD",
    );
    assert_rgb_near(
        flash.at(corner.0, corner.1),
        flashed,
        1,
        "only flash: the corner shows no vignette",
    );

    // Vignette: strong at the corner, near zero at the centre; no flash.
    let vignette = run(CoversHud {
        vignette: true,
        ..CoversHud::default()
    })
    .expect("adapter present");
    let corner_uv = (0.5 / 14.0 - 0.5_f32, 0.5 / 8.0 - 0.5_f32);
    let factor = ((corner_uv.0 * corner_uv.0 + corner_uv.1 * corner_uv.1) * 2.0).min(1.0);
    let vignetted: [u8; 3] = std::array::from_fn(|c| {
        let target = if c == 2 { 1.0 } else { 0.0 };
        linear_to_srgb(lin(c) * (1.0 - factor) + target * factor)
    });
    assert_rgb_near(
        vignette.at(corner.0, corner.1),
        vignetted,
        1,
        "vignette covers the HUD",
    );
    assert!(
        vignette.at(centre.0, centre.1)[1].abs_diff(opaque[1]) <= 2,
        "only vignette: the centre shows no green flash"
    );

    // Shake: the UI moves with the scene, untinted.
    let shake = run(CoversHud {
        shake: true,
        ..CoversHud::default()
    })
    .expect("adapter present");
    // A 0.1-screen shake samples the layer 1.4 px right and 0.8 px down, so
    // the centre texel lands one pixel up and to the left of where it was drawn.
    assert_eq!(
        shake.at(6, 3),
        opaque,
        "only shake: the moved UI is untinted"
    );
    assert_ne!(
        shake.at(centre.0, centre.1),
        opaque,
        "the UI no longer sits where it was drawn"
    );
}
