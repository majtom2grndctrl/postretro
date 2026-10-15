//! Adapter-backed fluid surface draw through the production capture recorder.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::{ClearColor, FluidSurfaceDraw, Renderer, ShSampleRegionSets};
use glam::{Mat4, Vec2, Vec3, Vec4};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_LOBE_COUNT, ReflectionProbeBinding,
    ReflectionProbeLightShare, ReflectionProbeRecord, ReflectionProbesSection,
};
use postretro_level_format::sh_volume::AnimationDescriptor;
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_REFLECTION_ALL_TERMS, FLUID_REFLECTION_NO_PROBE, FluidReflectionFragment,
    FluidReflectionLights, reflection_fragment,
};
use postretro_render_cpu::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_SMOOTH, FluidReflectionStyle, FluidTexelAxes,
};
use postretro_render_cpu::fluid_surface::{
    FLUID_PARAMS_WINDOW_BYTES, FLUID_SLOTS_PER_WINDOW, FluidSurfaceLook, slot_window,
};
use postretro_render_cpu::render_extent::{Extent, RenderResolutionPolicy};
use postretro_render_cpu::sh_volume::{
    ANIMATION_DESCRIPTOR_SIZE, build_animation_buffers_from_descriptors,
};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const SIDE: u32 = 16;
const WHITE: ClearColor = ClearColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const ALL: VisibleCells = VisibleCells::DrawAll;

fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(SIDE, SIDE) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[FluidSurfaceProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

fn clear_draw(volume_index: u32, opacity: f32) -> FluidSurfaceDraw {
    look_draw(volume_index, FluidSurfaceLook::surface_only(opacity))
}

fn look_draw(volume_index: u32, look: FluidSurfaceLook) -> FluidSurfaceDraw {
    FluidSurfaceDraw {
        volume_index,
        params: look.pack(),
    }
}

/// A screen-height quad spanning clip x `x0..x1` at clip depth 0.5 under an
/// identity view-projection, its outward normal toward the capture camera at
/// z = −1, so it is seen from outside its volume.
fn fluid_quad(x0: f32, x1: f32) -> EnvironmentVolumeRecord {
    // Texel UVs over one tile of the 64-texel placeholder checkerboard.
    let corner = |x: f32, y: f32| EnvironmentSurfaceVertex {
        position: [x, y, 0.5],
        uv: [(x + 1.0) * 32.0, (y + 1.0) * 32.0],
    };
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: vec![
                corner(x0, -1.0),
                corner(x0, 1.0),
                corner(x1, 1.0),
                corner(x1, -1.0),
            ],
        }],
    }
}

fn full_screen_fluid() -> EnvironmentVolumeRecord {
    fluid_quad(-1.0, 1.0)
}

/// The same quad wound the other way: its outward normal points away from
/// the camera, so the camera sees it from inside its volume.
fn full_screen_fluid_from_inside() -> EnvironmentVolumeRecord {
    let mut volume = full_screen_fluid();
    volume.surface_faces[0].vertices.reverse();
    volume
}

/// One captured frame's RGBA8 pixels at the renderer's capture extent.
fn capture(renderer: &mut Renderer) -> Vec<u8> {
    capture_from(renderer, Vec3::new(0.0, 0.0, -1.0))
}

/// [`capture`] with the shading camera at `camera`; the identity
/// view-projection still frames the quads, so only lighting, fog and
/// reflection see the move.
fn capture_from(renderer: &mut Renderer, camera: Vec3) -> Vec<u8> {
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &ALL,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &ALL,
                fog_cells: &[],
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            camera,
            &[],
            &[],
            1.0,
            WHITE,
            true,
            ShDrainBatch::default(),
        )
        .unwrap()
        .frame
        .unwrap()
}

/// Mean red channel of one captured frame over its left and right halves.
fn capture_mean_red_halves(renderer: &mut Renderer) -> (f32, f32) {
    let pixels = capture(renderer);
    let (mut left, mut right) = (0u32, 0u32);
    for (index, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if (index as u32 % SIDE) < SIDE / 2 {
            left += u32::from(px[0]);
        } else {
            right += u32::from(px[0]);
        }
    }
    let half_count = (pixels.len() / 8) as f32;
    (left as f32 / half_count, right as f32 / half_count)
}

fn capture_mean_red(renderer: &mut Renderer) -> f32 {
    let (left, right) = capture_mean_red_halves(renderer);
    (left + right) * 0.5
}

/// `screen_effects.wgsl`'s `soft_knee_tonemap` knee: identity at or below
/// this peak channel.
const CAPTURE_KNEE_START: f32 = 0.98;

/// The capture's encoding of one linear HDR scene colour: the resolve
/// shader's soft-knee tonemap, then the `Rgba8UnormSrgb` capture target's
/// sRGB transfer. Pinned to both by
/// `capture_encode_mirrors_the_soft_knee_tonemap_and_srgb_target`.
fn capture_encode(linear: Vec3) -> [u8; 3] {
    let peak = linear.max_element();
    let toned = if peak <= CAPTURE_KNEE_START {
        linear
    } else {
        let width = 1.0 - CAPTURE_KNEE_START;
        let excess = peak - CAPTURE_KNEE_START;
        linear * ((CAPTURE_KNEE_START + width * excess / (excess + width)) / peak)
    };
    toned.to_array().map(linear_to_srgb)
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

/// World position of capture pixel (`column`, `row`)'s centre on a
/// [`fluid_quad`] face: clip depth 0.5 under the identity view-projection,
/// row 0 at the top.
fn pixel_world(column: u32, row: u32) -> Vec3 {
    let step = 2.0 / SIDE as f32;
    Vec3::new(
        (column as f32 + 0.5) * step - 1.0,
        1.0 - (row as f32 + 0.5) * step,
        0.5,
    )
}

/// A probe term as the CPU reference evaluates it: the level's probes, each
/// array layer's uniform capture colour, and the row's reflectivity.
struct ReferenceProbes<'a> {
    section: &'a ReflectionProbesSection,
    layer_colours: &'a [Vec3],
    reflectivity: f32,
}

/// What the capture reads at (`column`, `row`) from the CPU references alone:
/// a smooth-style [`fluid_quad`] fragment drawing the straight-alpha
/// `surface`, its probe term (`reflection_fragment`, with no animated
/// lights) when `probe` names one, blended straight-alpha over the white
/// clear, then [`capture_encode`]d.
fn expected_pixel(
    camera: Vec3,
    column: u32,
    row: u32,
    surface: Vec4,
    probe: u32,
    probes: &ReferenceProbes<'_>,
) -> [u8; 3] {
    let fragment = FluidReflectionFragment {
        probe,
        camera,
        world_position: pixel_world(column, row),
        outward: Vec3::NEG_Z,
        light_term_mask: FLUID_REFLECTION_ALL_TERMS,
        reflectivity: probes.reflectivity,
        style: FLUID_REFLECTION_STYLE_SMOOTH,
        texel_uv: Vec2::ZERO,
        axes: FluidTexelAxes::default(),
        time: 0.0,
        texels_per_pixel: 0.0,
    };
    let out = reflection_fragment(
        surface,
        &fragment,
        &probes.section.probes,
        [probes.section.tile_dimension, probes.section.tile_border],
        &FluidReflectionLights::NONE,
        |layer, _| probes.layer_colours[layer as usize],
    );
    capture_encode(out.truncate() * out.w + Vec3::ONE * (1.0 - out.w))
}

/// The largest per-channel deviation of `pixels` from `expected` over
/// `columns`, every row, and the pixel it occurs at.
fn worst_deviation(
    pixels: &[u8],
    columns: std::ops::Range<u32>,
    expected: impl Fn(u32, u32) -> [u8; 3],
) -> (u8, (u32, u32)) {
    let mut worst = (0, (0, 0));
    for row in 0..SIDE {
        for column in columns.clone() {
            let at = ((row * SIDE + column) * 4) as usize;
            let want = expected(column, row);
            for channel in 0..3 {
                let deviation = pixels[at + channel].abs_diff(want[channel]);
                if deviation > worst.0 {
                    worst = (deviation, (column, row));
                }
            }
        }
    }
    worst
}

/// A probe section over one fluid volume: probe `p` samples layer
/// `layers[p]`, each layer's tile the uniform f16 colour `tiles[layer]`, and
/// `bindings` are (face, probe) pairs of volume 0.
fn uniform_probe_section(
    dimension: u32,
    layers: &[u32],
    tiles: &[[u16; 3]],
    bindings: &[(u32, u32)],
) -> ReflectionProbesSection {
    assert_eq!(layers.len(), tiles.len());
    ReflectionProbesSection {
        tile_dimension: dimension,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: layers
            .iter()
            .map(|&layer| ReflectionProbeRecord {
                source_entity: 0,
                box_min: [-8.0; 3],
                box_max: [8.0; 3],
                capture_point: [0.0, 0.0, 0.0],
                layer,
                lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
            })
            .collect(),
        bindings: bindings
            .iter()
            .map(|&(face, probe)| ReflectionProbeBinding {
                volume: 0,
                face,
                probe,
            })
            .collect(),
        tiles: tiles
            .iter()
            .flat_map(|&rgb| uniform_rgba16f_tile(dimension, rgb))
            .collect(),
    }
}

/// Linear RGB of f16 bits, for exact values (0, powers of two, small sums).
fn f16_rgb(bits: [u16; 3]) -> Vec3 {
    Vec3::from(bits.map(|half| {
        let exponent = i32::from((half >> 10) & 0x1F) - 15;
        let mantissa = f32::from(half & 0x3FF) / 1024.0;
        if half == 0 {
            0.0
        } else {
            (1.0 + mantissa) * 2f32.powi(exponent)
        }
    }))
}

/// The capture pins the resolve's soft knee and the sRGB target that
/// [`capture_encode`] mirrors.
#[test]
fn capture_encode_mirrors_the_soft_knee_tonemap_and_srgb_target() {
    let shader: String = include_str!("../shaders/screen_effects.wgsl")
        .split_whitespace()
        .collect();
    for line in [
        "letpeak=max(max(color.r,color.g),color.b);",
        "letknee_start=0.98;",
        "ifpeak<=knee_start{returncolor;}",
        "letknee_width=1.0-knee_start;",
        "letexcess=peak-knee_start;",
        "letcompressed_peak=knee_start+knee_width*excess/(excess+knee_width);",
        "returncolor*(compressed_peak/peak);",
        "varcolor=soft_knee_tonemap(scene.rgb);",
    ] {
        assert!(shader.contains(line), "screen_effects.wgsl lost `{line}`");
    }
    let effects: String = include_str!("screen_effects.rs")
        .split_whitespace()
        .collect();
    let capture = &effects[effects
        .find("fncreate_capture_color(")
        .expect("capture target")..];
    let body = &capture[..capture.find("})").expect("descriptor closes")];
    assert!(
        body.contains("format:wgpu::TextureFormat::Rgba8UnormSrgb,"),
        "{body}"
    );
    // Linear 0.5 sits below the knee: 188 of 255.
    assert_eq!(capture_encode(Vec3::splat(0.5)), [188; 3]);
    assert_eq!(f16_rgb([0x3800, 0x3C00, 0x4200]), Vec3::new(0.5, 1.0, 3.0));
}

/// A full-screen face whose UVs span texels 3.5..4.5 of the placeholder,
/// inside its first 8-texel magenta square: shimmer and rings move the sample
/// at most two texels, so every fragment's albedo is linear (1, 0, 1).
fn magenta_fluid() -> EnvironmentVolumeRecord {
    let mut volume = full_screen_fluid();
    for vertex in &mut volume.surface_faces[0].vertices {
        vertex.uv = [vertex.position[0], vertex.position[1]].map(|clip| 3.5 + (clip + 1.0) * 0.5);
    }
    volume
}

// Acceptance row 35. The pass builds, binds its material and parameter
// window, and draws: an unlit fluid face (no SH, no lights, zero ambient
// floor) at opacity 0.5 blends black over the white clear to linear 0.5,
// below the tonemap knee, so every pixel reads 188 ± 1. No draw leaves the
// clear untouched. A face lit by the ambient floor reads the capture's
// encoding of today's straight-alpha value within 1/255: its magenta albedo
// at alpha 0.5 over white is (1, 0.5, 1), whose red peak the soft knee
// compresses.
#[test]
fn fluid_surface_blends_over_the_scene_at_its_surface_opacity() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()], None);

    renderer.set_fluid_surface_draws(&[]);
    let clear = capture_mean_red(&mut renderer);

    renderer.set_fluid_surface_draws(&[clear_draw(0, 0.5)]);
    let half = capture(&mut renderer);

    renderer.set_fluid_surface_draws(&[clear_draw(0, 1.0)]);
    let opaque = capture_mean_red(&mut renderer);

    let (half_low, half_high) = half
        .as_chunks::<4>()
        .0
        .iter()
        .fold((u8::MAX, 0), |(low, high), px| {
            (low.min(px[0]), high.max(px[0]))
        });
    eprintln!(
        "[FluidSurfaceProof] row 35 GPU case ran: clear={clear} half red {half_low}..={half_high} \
         opaque={opaque}"
    );
    // White sits past the knee, so it reads 254, not 255.
    let white = f32::from(capture_encode(Vec3::ONE)[0]);
    assert!(
        (clear - white).abs() < 0.5,
        "the white clear reads its encoding {white}, got {clear}"
    );
    assert!(opaque < 1.0, "an unlit opaque face is black, got {opaque}");
    assert!(
        half_low >= 187 && half_high <= 189,
        "an unlit face at opacity 0.5 over white reads 188 ± 1 red, got {half_low}..={half_high}"
    );

    // Lit by the ambient floor, the face shows its material.
    renderer.install_fluid_surfaces(&[magenta_fluid()], None);
    renderer.set_ambient_floor(1.0);
    renderer.set_fluid_surface_draws(&[clear_draw(0, 0.5)]);
    let lit = capture(&mut renderer);
    let alpha = 0.5;
    let today = Vec3::new(1.0, 0.0, 1.0) * alpha + Vec3::ONE * (1.0 - alpha);
    let want = capture_encode(today);
    let (worst, at) = worst_deviation(&lit, 0..SIDE, |_, _| want);
    eprintln!(
        "[FluidSurfaceProof] row 35 lit GPU case ran: expected {want:?}, worst deviation {worst} \
         at {at:?}"
    );
    assert!(
        worst <= 1,
        "a lit unbound face reads the encoding {want:?} of today's value within 1/255, \
         off by {worst} at {at:?}"
    );
    renderer.set_ambient_floor(0.0);

    // Unloading drops the geometry and closes the pass.
    renderer.release_level_resources();
    renderer.set_fluid_surface_draws(&[clear_draw(0, 1.0)]);
    assert!(!renderer.full().fluid_surface.has_draws());
}

// Each draw reads its own volume's opacity: the parameter row is addressed by
// the draw's first instance, so a second volume must not read the first's.
#[test]
fn fluid_surface_draws_read_each_volumes_own_opacity_slot() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // A dry volume between them takes no slot, so volume 2 is slot 1.
    let dry = EnvironmentVolumeRecord {
        surface_faces: Vec::new(),
        ..fluid_quad(0.0, 1.0)
    };
    renderer.install_fluid_surfaces(&[fluid_quad(-1.0, 0.0), dry, fluid_quad(0.0, 1.0)], None);
    renderer.set_fluid_surface_draws(&[clear_draw(2, 0.5), clear_draw(0, 1.0)]);
    let (left, right) = capture_mean_red_halves(&mut renderer);
    eprintln!("[FluidSurfaceProof] per-volume opacity: left={left} right={right}");
    assert!(left < 1.0, "volume 0 is opaque and unlit, got {left}");
    assert!(
        right > 100.0 && right < 240.0,
        "volume 2 is half transparent, got {right}"
    );
}

// Pin P1: a level whose draws span two params windows gives every volume its
// own row. The first window's last slot and the second window's first slot
// draw in one frame at distinct opacities, each read back from the pixels;
// the second window binds at a dynamic offset that is a multiple of 256.
#[test]
fn fluid_surface_draws_across_two_params_windows_read_their_own_rows() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let last_of_first = FLUID_SLOTS_PER_WINDOW - 1;
    let first_of_second = FLUID_SLOTS_PER_WINDOW;
    assert_eq!(slot_window(last_of_first), (0, FLUID_SLOTS_PER_WINDOW - 1));
    assert_eq!(slot_window(first_of_second), (1, 0));
    let alignment = u64::from(renderer.device.limits().min_uniform_buffer_offset_alignment);
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES % 256, 0);
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES % alignment, 0);

    // What each opacity reads as on the right half from a one-window level
    // (the capture tonemaps, so 0.5 is not mid-grey).
    let mut reference = |opacity: f32| {
        renderer.install_fluid_surfaces(&[fluid_quad(-1.0, 0.0), fluid_quad(0.0, 1.0)], None);
        renderer.set_fluid_surface_draws(&[clear_draw(1, opacity)]);
        capture_mean_red_halves(&mut renderer).1
    };
    let (half, light, dark) = (reference(0.5), reference(0.2), reference(0.9));
    assert!(
        (half - light).abs() > 10.0 && (half - dark).abs() > 10.0,
        "reference opacities must read apart: 0.5={half} 0.2={light} 0.9={dark}"
    );

    // Every volume draws a face, so volume i is slot i. All but the two under
    // test sit off screen.
    let volumes: Vec<EnvironmentVolumeRecord> = (0..=first_of_second + 1)
        .map(|volume| match volume {
            v if v == last_of_first => fluid_quad(-1.0, 0.0),
            v if v == first_of_second => fluid_quad(0.0, 1.0),
            _ => fluid_quad(3.0, 3.5),
        })
        .collect();
    renderer.install_fluid_surfaces(&volumes, None);
    // Slot 0 and the second window's second slot carry other opacities, so a
    // draw that read the wrong window or element would show.
    renderer.set_fluid_surface_draws(&[
        clear_draw(0, 0.2),
        clear_draw(last_of_first, 1.0),
        clear_draw(first_of_second, 0.5),
        clear_draw(first_of_second + 1, 0.9),
    ]);
    let (left, right) = capture_mean_red_halves(&mut renderer);
    eprintln!(
        "[FluidSurfaceProof] P1 two-window GPU case ran: slot {last_of_first} (window 0) \
         left={left}, slot {first_of_second} (window 1, offset {FLUID_PARAMS_WINDOW_BYTES}) \
         right={right} (one-window reference {half})"
    );
    assert!(left < 1.0, "window 0's last slot is opaque, got {left}");
    assert!(
        (right - half).abs() < 2.0,
        "window 1's first slot reads its own 0.5 row ({half}), got {right}"
    );
}

// A fogged fluid seen from outside draws murk over the scene behind it, even
// at surface opacity 0; the eye fog weight scales it, defaulting to 1 before
// any write; from inside the volume, or with fog omitted, it draws nothing.
#[test]
fn fluid_depth_fog_draws_from_outside_and_scales_with_the_eye_weight() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    assert_eq!(
        renderer.full().fluid_surface.scene_inputs().fog_weight(),
        1.0
    );
    // Opacity 0, fog distance 0.5: under the identity camera the path to the
    // cleared far depth is 0.5 at the centre, so transmittance is about 0.05
    // there; the face is unlit, so the murk is black over the white clear.
    let murk = FluidSurfaceLook {
        fog_distance: Some(0.5),
        fog_color: [0.5, 0.5, 0.5],
        ..FluidSurfaceLook::surface_only(0.0)
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()], None);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let full = capture_mean_red(&mut renderer);

    renderer.set_fluid_fog_weight(0.5);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let half = capture_mean_red(&mut renderer);

    renderer.set_fluid_fog_weight(0.0);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let none = capture_mean_red(&mut renderer);

    // The capture tonemaps and encodes sRGB, so a ~3% linear remainder of the
    // white scene reads near 50 of 255.
    eprintln!("[FluidSurfaceProof] depth fog GPU case ran: weight 1={full} 0.5={half} 0={none}");
    assert!(full < 70.0, "full murk hides the white scene, got {full}");
    assert!(
        half > full + 60.0 && half < none - 30.0,
        "half weight draws half the murk, got {half} between {full} and {none}"
    );
    assert!(none > 250.0, "weight 0 draws no murk, got {none}");

    // Seen from inside, the same fluid draws no fog whatever the weight.
    renderer.set_fluid_fog_weight(1.0);
    renderer.install_fluid_surfaces(&[full_screen_fluid_from_inside()], None);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let inside = capture_mean_red(&mut renderer);
    eprintln!("[FluidSurfaceProof] depth fog seen from inside: {inside}");
    assert!(inside > 250.0, "no fog from inside, got {inside}");
}

// A mid-level drift re-resolve rewrites only the drift buffer. The next
// frame still draws at the same opacity, and a later frame with unchanged
// opacities uploads no parameter rows.
#[test]
fn fluid_drift_re_resolve_keeps_draws_and_uploads_no_parameter_rows() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()], None);
    let draws = [clear_draw(0, 0.5)];
    renderer.set_fluid_surface_draws(&draws);
    let before = capture_mean_red(&mut renderer);
    assert_eq!(renderer.full().fluid_surface.param_uploads(), Some(1));

    renderer.set_fluid_surface_drift(&[Vec3::new(3.0, 0.0, 0.0)]);
    renderer.set_fluid_surface_draws(&draws);
    let after = capture_mean_red(&mut renderer);
    renderer.set_fluid_surface_draws(&draws);
    let later = capture_mean_red(&mut renderer);

    eprintln!("[FluidSurfaceProof] re-resolve: before={before} after={after} later={later}");
    assert!(
        (after - before).abs() < 2.0 && (later - before).abs() < 2.0,
        "the pool keeps its opacity across a re-resolve: {before} → {after} → {later}"
    );
    assert_eq!(
        renderer.full().fluid_surface.param_uploads(),
        Some(1),
        "unchanged opacities upload no parameter rows"
    );
}

fn scene_inputs_extent(renderer: &Renderer) -> (Extent, u32) {
    let (view, builds) = renderer.full().fluid_surface.scene_inputs().scene_depth();
    let texture = view.texture();
    (Extent::new(texture.width(), texture.height()), builds)
}

// Pin P4: group 5 exists from full init without a level, follows the scene
// depth through every resize with or without a level installed, and level
// install and release never rebuild it; the frame after a resize draws fog
// over the new depth.
#[test]
fn fluid_scene_inputs_follow_the_scene_depth_across_resizes_with_and_without_a_level() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (extent, built) = scene_inputs_extent(&renderer);
    assert_eq!(
        extent,
        renderer.scene_extent(),
        "built at full init, no level"
    );

    renderer.record_surface_size(40, 24);
    renderer.commit_extents().expect("the surface changed");
    assert_eq!(
        scene_inputs_extent(&renderer),
        (Extent::new(40, 24), built + 1),
        "resize without a level"
    );

    renderer.install_fluid_surfaces(&[full_screen_fluid()], None);
    assert_eq!(
        scene_inputs_extent(&renderer).1,
        built + 1,
        "level install leaves group 5 alone"
    );

    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    renderer.record_surface_size(64, 48);
    renderer.commit_extents().expect("both extents changed");
    let scene = renderer.scene_extent();
    assert_eq!(scene, Extent::new(32, 24));
    assert_eq!(
        scene_inputs_extent(&renderer),
        (scene, built + 2),
        "resize with a level"
    );

    // Capture pins divisor 1, one more scene rebuild; the fogged frame after
    // it draws over the new depth.
    let murk = FluidSurfaceLook {
        fog_distance: Some(0.5),
        ..FluidSurfaceLook::surface_only(0.0)
    };
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let pixels = capture(&mut renderer);
    let scene = renderer.scene_extent();
    assert_eq!(scene, Extent::new(64, 48));
    assert_eq!(pixels.len(), (64 * 48 * 4) as usize);
    let mean_red = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| f32::from(px[0]))
        .sum::<f32>()
        / (64.0 * 48.0);
    assert_eq!(scene_inputs_extent(&renderer), (scene, built + 3));
    assert!(
        mean_red < 70.0,
        "fog draws after the resize, got {mean_red}"
    );

    renderer.release_level_resources();
    assert_eq!(
        scene_inputs_extent(&renderer),
        (scene, built + 3),
        "level release leaves group 5 alone"
    );
    eprintln!(
        "[FluidSurfaceProof] P4 scene-inputs GPU case ran: group 5 at {}x{} after {} builds, \
         mean red {mean_red}",
        scene.width,
        scene.height,
        built + 3
    );
}

/// Mean RGB of one captured frame over its left and right halves.
fn mean_rgb_halves(pixels: &[u8]) -> ([f32; 3], [f32; 3]) {
    let (mut left, mut right) = ([0u32; 3], [0u32; 3]);
    for (index, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let half = if (index as u32 % SIDE) < SIDE / 2 {
            &mut left
        } else {
            &mut right
        };
        for channel in 0..3 {
            half[channel] += u32::from(px[channel]);
        }
    }
    let count = (pixels.len() / 8) as f32;
    (
        left.map(|sum| sum as f32 / count),
        right.map(|sum| sum as f32 / count),
    )
}

/// One RGBA16F probe tile `dimension`² of the uniform linear colour `rgb`.
fn uniform_rgba16f_tile(dimension: u32, rgb: [u16; 3]) -> Vec<u8> {
    let texel = [rgb[0], rgb[1], rgb[2], 0x3C00];
    (0..dimension * dimension)
        .flat_map(|_| texel.iter().flat_map(|half| half.to_le_bytes()))
        .collect()
}

/// The shading camera high off the faces' outward (−Z) side, about 72° from
/// the normal at the centre: a grazing view. The identity view-projection
/// still frames the quads.
const GRAZING: Vec3 = Vec3::new(0.0, 3.0, -0.5);

/// An unlit face at opacity 0.5: black at half coverage, so today's value
/// over the white clear is linear 0.5.
const UNLIT_HALF: Vec4 = Vec4::new(0.0, 0.0, 0.0, 0.5);

/// The linear scene colour the CPU references give a smooth-style fragment
/// of probe 0 at the centre column of `row`, before the capture encodes it.
fn reference_linear(
    row: u32,
    section: &ReflectionProbesSection,
    colours: &[Vec3],
    reflectivity: f32,
) -> Vec3 {
    let fragment = FluidReflectionFragment {
        probe: 0,
        camera: GRAZING,
        world_position: pixel_world(SIDE / 2, row),
        outward: Vec3::NEG_Z,
        light_term_mask: FLUID_REFLECTION_ALL_TERMS,
        reflectivity,
        style: FLUID_REFLECTION_STYLE_SMOOTH,
        texel_uv: Vec2::ZERO,
        axes: FluidTexelAxes::default(),
        time: 0.0,
        texels_per_pixel: 0.0,
    };
    let out = reflection_fragment(
        UNLIT_HALF,
        &fragment,
        &section.probes,
        [section.tile_dimension, section.tile_border],
        &FluidReflectionLights::NONE,
        |layer, _| colours[layer as usize],
    );
    out.truncate() * out.w + Vec3::ONE * (1.0 - out.w)
}

// Acceptance row 33 (P3). A full-screen face bound to a probe whose capture
// is a uniform colour R, with no animated lights, read at a fixed grazing
// view in smooth style, reads the capture's encoding of F·R + (1−F)·(surface
// over clear) within 2/255 at every pixel, F being the CPU Fresnel reference
// at that pixel's view angle. R is bright enough in red that the lower rows
// cross the soft knee while the upper rows stay below it, and dim enough in
// luminance that bloom extracts nothing. At reflectivity 0 every pixel reads
// the encoding of today's value.
#[test]
fn a_uniform_capture_reads_its_fresnel_composite_in_the_capture_encoding() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    // f16 (3, 0.25, 0.5).
    let r_bits = [0x4200, 0x3400, 0x3800];
    let section = uniform_probe_section(8, &[0], &[r_bits], &[(0, 0)]);
    let colours = [f16_rgb(r_bits)];
    assert_eq!(colours[0], Vec3::new(3.0, 0.25, 0.5));
    let pool = full_screen_fluid();
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(&section));

    // The view crosses the knee, and bloom (luminance threshold) sees nothing.
    let top = reference_linear(0, &section, &colours, 1.0);
    let bottom = reference_linear(SIDE - 1, &section, &colours, 1.0);
    assert!(
        top.max_element() < CAPTURE_KNEE_START && bottom.max_element() > 1.0,
        "the view crosses the knee: top {top}, bottom {bottom}"
    );
    assert!(
        bottom.dot(Vec3::new(0.2126, 0.7152, 0.0722)) < super::bloom::BLOOM_THRESHOLD,
        "bloom extracts nothing: {bottom}"
    );

    for reflectivity in [1.0, 0.0] {
        let look = FluidSurfaceLook {
            surface_reflectivity: reflectivity,
            has_bound_face: true,
            ..FluidSurfaceLook::surface_only(0.5)
        };
        renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
        let pixels = capture_from(&mut renderer, GRAZING);
        let reference = ReferenceProbes {
            section: &section,
            layer_colours: &colours,
            reflectivity,
        };
        let (worst, at) = worst_deviation(&pixels, 0..SIDE, |column, row| {
            expected_pixel(GRAZING, column, row, UNLIT_HALF, 0, &reference)
        });
        let centre = ((SIDE / 2 * SIDE + SIDE / 2) * 4) as usize;
        eprintln!(
            "[FluidSurfaceProof] row 33 GPU case ran: reflectivity {reflectivity}, worst \
             deviation {worst} at {at:?}, centre {:?}, top-left {:?}, bottom-left {:?}",
            &pixels[centre..centre + 3],
            &pixels[..3],
            &pixels[((SIDE - 1) * SIDE * 4) as usize..][..3],
        );
        assert!(
            worst <= 2,
            "reflectivity {reflectivity}: off the encoded composite by {worst} at {at:?}"
        );
        if reflectivity > 0.0 {
            assert!(
                pixels[..3] != [188; 3],
                "the probe term shows: {:?}",
                &pixels[..4]
            );
        } else {
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|px| px[..3] == [188; 3]),
                "reflectivity 0 reads today's linear 0.5 everywhere"
            );
        }
    }
    renderer.release_level_resources();
}

// Acceptance row 34. One volume, three faces in one draw: the left quarter
// binds probe 0, the next quarter probe 1, the right half nothing. The
// probes sample distinct uniform captures, stored in swapped layers so the
// layer comes from the probe table. Each bound face reads the encoding of
// its own probe's composite within 2/255; the unbound face reads today's
// value, exactly the frame drawn with no section 54. Seen from inside the
// volume every face reads today's value. The probe array is level-scoped: a
// level without the section binds the 1-layer dummy, and unload drops it.
#[test]
fn probe_bound_faces_read_their_own_probes_and_an_unbound_face_in_the_draw_reads_today() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let mut pool = fluid_quad(-1.0, -0.5);
    pool.surface_faces
        .extend(fluid_quad(-0.5, 0.0).surface_faces);
    pool.surface_faces
        .extend(fluid_quad(0.0, 1.0).surface_faces);
    // Probe 0 is green (layer 1), probe 1 red with some blue (layer 0).
    let (green, red) = ([0, 0x4000, 0], [0x4000, 0, 0x3800]);
    let section = uniform_probe_section(8, &[1, 0], &[red, green], &[(0, 0), (1, 1)]);
    let colours = [f16_rgb(red), f16_rgb(green)];
    let reference = ReferenceProbes {
        section: &section,
        layer_colours: &colours,
        reflectivity: 1.0,
    };
    let look = FluidSurfaceLook {
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.5)
    };

    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), None);
    assert_eq!(renderer.full().fluid_surface.probe_array_layers(), Some(1));
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let today = capture_from(&mut renderer, GRAZING);

    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(&section));
    assert_eq!(renderer.full().fluid_surface.probe_array_layers(), Some(2));
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let pixels = capture_from(&mut renderer, GRAZING);
    let face_probe = |column: u32| match column {
        0..4 => 0,
        4..8 => 1,
        _ => FLUID_REFLECTION_NO_PROBE,
    };
    let expected = |column, row| {
        expected_pixel(
            GRAZING,
            column,
            row,
            UNLIT_HALF,
            face_probe(column),
            &reference,
        )
    };
    let (probe_0, at_0) = worst_deviation(&pixels, 0..4, expected);
    let (probe_1, at_1) = worst_deviation(&pixels, 4..8, expected);
    let (unbound, at_unbound) = worst_deviation(&pixels, 8..SIDE, expected);
    let (unbound_today, _) = worst_deviation(&pixels, 8..SIDE, |column, row| {
        let at = ((row * SIDE + column) * 4) as usize;
        [today[at], today[at + 1], today[at + 2]]
    });
    let centre = |column: u32| {
        let at = ((SIDE / 2 * SIDE + column) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2]]
    };
    eprintln!(
        "[FluidSurfaceProof] row 34 GPU case ran: probe 0 {:?} worst {probe_0} at {at_0:?}, \
         probe 1 {:?} worst {probe_1} at {at_1:?}, unbound {:?} worst {unbound} at \
         {at_unbound:?} (vs the no-section frame {unbound_today})",
        centre(1),
        centre(5),
        centre(12)
    );
    assert!(
        probe_0 <= 2 && probe_1 <= 2,
        "each face reads its own probe"
    );
    assert!(
        centre(1)[1] > centre(1)[0] + 20 && centre(5)[0] > centre(5)[1] + 20,
        "probe 0 reads green, probe 1 red: {:?} {:?}",
        centre(1),
        centre(5)
    );
    assert!(
        unbound <= 1 && unbound_today == 0,
        "the unbound face reads today's value"
    );

    // The same faces seen from inside their volume (from +Z) reflect nothing.
    let below = Vec3::new(0.0, 3.0, 1.5);
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), None);
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let today_below = capture_from(&mut renderer, below);
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(&section));
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let bound_below = capture_from(&mut renderer, below);
    assert!(
        bound_below == today_below,
        "from below every face reads today's value"
    );

    renderer.release_level_resources();
    assert_eq!(renderer.full().fluid_surface.probe_array_layers(), None);
}

// Pin P11, level change with probes bound. Level A (section 54, three 16²
// tiles) draws its probe's capture. Unload leaves no probe texture. Level B
// has fluid and no section 54: it binds only the 1×1 one-layer dummy, and
// its face, even drawn as a mirror claiming a bound face, reads exactly
// today's value, so nothing of A's samples. Level B2 brings its own section
// (two 8² tiles) whose layer 0 differs from A's: its face reads B2's capture,
// not A's, and its array is B2's size. Unload leaves no probe texture again.
#[test]
fn a_level_change_binds_only_the_new_levels_probes_and_unload_leaves_none() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let pool = full_screen_fluid();
    let mirror = FluidSurfaceLook {
        surface_reflectivity: 50.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.5)
    };
    let size = |renderer: &Renderer| {
        renderer
            .full()
            .fluid_surface
            .probe_array_size()
            .map(|size| (size.width, size.height, size.depth_or_array_layers))
    };
    let draw = |renderer: &mut Renderer, section: Option<&ReflectionProbesSection>| {
        renderer.install_fluid_surfaces(std::slice::from_ref(&pool), section);
        renderer.set_fluid_surface_draws(&[look_draw(0, mirror)]);
        capture_from(renderer, GRAZING)
    };
    let worst = |pixels: &[u8], section: &ReflectionProbesSection, colours: &[Vec3]| {
        let reference = ReferenceProbes {
            section,
            layer_colours: colours,
            reflectivity: 50.0,
        };
        worst_deviation(pixels, 0..SIDE, |column, row| {
            expected_pixel(GRAZING, column, row, UNLIT_HALF, 0, &reference)
        })
    };

    // Level A: red, green, blue layers; the face binds probe 0 (layer 0, red).
    let a_tiles = [[0x3800, 0, 0], [0, 0x3800, 0], [0, 0, 0x3800]];
    let a = uniform_probe_section(16, &[0, 1, 2], &a_tiles, &[(0, 0)]);
    let a_pixels = draw(&mut renderer, Some(&a));
    assert_eq!(size(&renderer), Some((16, 16, 3)));
    let (a_worst, a_at) = worst(&a_pixels, &a, &a_tiles.map(f16_rgb));
    assert!(
        a_worst <= 2,
        "level A reads its capture: off by {a_worst} at {a_at:?}"
    );
    assert!(
        a_pixels[0] > a_pixels[1] + 20,
        "level A reflects red: {:?}",
        &a_pixels[..4]
    );

    renderer.release_level_resources();
    assert_eq!(size(&renderer), None, "unload leaves no probe texture");

    // Level B: fluid, no section 54.
    let b_pixels = draw(&mut renderer, None);
    assert_eq!(size(&renderer), Some((1, 1, 1)), "B binds only the dummy");
    assert!(
        b_pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[..3] == [188; 3]),
        "B draws fluid as today: linear 0.5 everywhere, got {:?}",
        &b_pixels[..4]
    );

    // Level B2: its own blue layer 0 and a yellow layer 1; the face binds
    // probe 0.
    renderer.release_level_resources();
    let b2_tiles = [[0, 0, 0x3800], [0x3800, 0x3800, 0]];
    let b2 = uniform_probe_section(8, &[0, 1], &b2_tiles, &[(0, 0)]);
    let b2_pixels = draw(&mut renderer, Some(&b2));
    assert_eq!(size(&renderer), Some((8, 8, 2)), "B2 binds its own array");
    let (b2_worst, b2_at) = worst(&b2_pixels, &b2, &b2_tiles.map(f16_rgb));
    assert!(
        b2_worst <= 2,
        "B2 reads its own capture: off by {b2_worst} at {b2_at:?}"
    );
    assert!(
        b2_pixels[2] > b2_pixels[0] + 20,
        "B2 reflects its blue, not A's red: {:?}",
        &b2_pixels[..4]
    );

    renderer.release_level_resources();
    assert_eq!(size(&renderer), None, "unload leaves no probe texture");
    eprintln!(
        "[FluidSurfaceProof] P11 level-change GPU case ran: A {:?} worst {a_worst}, B {:?}, \
         B2 {:?} worst {b2_worst}",
        &a_pixels[..3],
        &b_pixels[..3],
        &b2_pixels[..3]
    );
}

/// Descriptor record 0 for a flat light of `base_color`, on or off, as the
/// light bridge writes it into the descriptor mirror.
fn flat_descriptor(base_color: [f32; 3], start_active: bool) -> [u8; ANIMATION_DESCRIPTOR_SIZE] {
    let (bytes, _, _) = build_animation_buffers_from_descriptors(&[AnimationDescriptor {
        base_color,
        start_active: u32::from(start_active),
        ..AnimationDescriptor::default()
    }]);
    bytes[..ANIMATION_DESCRIPTOR_SIZE]
        .try_into()
        .expect("one descriptor record")
}

// Live lighting on an adapter. A probe's one share entry names roster light 0
// at reference (1, 1, 1) holding half the capture's light in every lobe. With
// the light's descriptor on at its reference the face reads its capture, as a
// probe with no lights does; switched off, it reads darker by exactly its
// share: the pixels of a capture of half the brightness with no lights. The
// descriptor reaches the GPU through the frame's one flush, which the capture
// recorder runs before the fluid pass.
#[test]
fn a_probe_reads_darker_by_its_lights_share_when_its_descriptor_switches_off() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let dimension = 8;
    let probes = |green: u16, light: ReflectionProbeLightShare| {
        let mut lights = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
        lights[0] = light;
        ReflectionProbesSection {
            tile_dimension: dimension,
            tile_border: 1,
            atlas_format: IRRADIANCE_FORMAT_RGBA16F,
            probes: vec![ReflectionProbeRecord {
                source_entity: 0,
                box_min: [-8.0; 3],
                box_max: [8.0; 3],
                capture_point: [0.0, 0.0, 0.0],
                layer: 0,
                lights,
            }],
            bindings: vec![ReflectionProbeBinding {
                volume: 0,
                face: 0,
                probe: 0,
            }],
            tiles: uniform_rgba16f_tile(dimension, [0, green, 0]),
        }
    };
    let half_share = ReflectionProbeLightShare {
        roster_index: 0,
        reference: [1.0; 3],
        shares: [[0.5; 3]; REFLECTION_PROBE_LOBE_COUNT],
    };
    // f16 1.0 and 0.5: linear green 1 and 0.5.
    let (one, half) = (0x3C00, 0x3800);
    let look = FluidSurfaceLook {
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.5)
    };
    let grazing = Vec3::new(0.0, 3.0, -0.5);
    let pool = full_screen_fluid();
    let draw = |renderer: &mut Renderer, section: &ReflectionProbesSection| {
        renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(section));
        renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
        mean_rgb_halves(&capture_from(renderer, grazing)).0
    };

    let unlit = draw(
        &mut renderer,
        &probes(one, ReflectionProbeLightShare::UNUSED),
    );
    let reference_half = draw(
        &mut renderer,
        &probes(half, ReflectionProbeLightShare::UNUSED),
    );

    let animation = &mut renderer.full_mut().sh_volume_resources.animation;
    animation.write_record_for_test(0, &flat_descriptor([1.0; 3], true));
    let on = draw(&mut renderer, &probes(one, half_share));

    let animation = &mut renderer.full_mut().sh_volume_resources.animation;
    animation.write_record_for_test(0, &flat_descriptor([1.0; 3], false));
    let off = draw(&mut renderer, &probes(one, half_share));

    eprintln!(
        "[FluidSurfaceProof] live-lighting GPU case ran: no lights {unlit:?}, light on {on:?}, \
         light off {off:?}, half capture {reference_half:?}"
    );
    assert!(
        unlit[1] > reference_half[1] + 10.0,
        "half the capture reads darker: {unlit:?} vs {reference_half:?}"
    );
    for channel in 0..3 {
        assert!(
            (on[channel] - unlit[channel]).abs() <= 0.5,
            "a light at its reference leaves the capture: {on:?} vs {unlit:?}"
        );
        assert!(
            (off[channel] - reference_half[channel]).abs() <= 1.0,
            "switched off, the face loses the light's share: {off:?} vs {reference_half:?}"
        );
    }
}

/// `value` (in `[0, 2)`, positive or zero) as f16 bits, mantissa truncated.
fn f16_bits(value: f32) -> u16 {
    if value <= 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    ((exponent as u16) << 10) | ((bits >> 13) & 0x3FF) as u16
}

/// One RGBA16F probe tile `dimension`² whose red follows the tile's `u` and
/// green its `v`, so every lookup direction reads its own colour.
fn gradient_rgba16f_tile(dimension: u32) -> Vec<u8> {
    let last = (dimension - 1) as f32;
    (0..dimension)
        .flat_map(|row| (0..dimension).map(move |column| (column, row)))
        .flat_map(|(column, row)| {
            [
                f16_bits(column as f32 / last),
                f16_bits(row as f32 / last),
                0,
                0x3C00,
            ]
        })
        .flat_map(|half| half.to_le_bytes())
        .collect()
}

/// Largest per-channel spread of the pixels in each 8 × 8 block of a
/// 16 × 16 capture: the four water texel cells of [`coarse_quad`].
fn cell_block_spreads(pixels: &[u8]) -> [u8; 4] {
    let mut spreads = [0u8; 4];
    for (block, spread) in spreads.iter_mut().enumerate() {
        let (bx, by) = ((block as u32 % 2) * 8, (block as u32 / 2) * 8);
        for channel in 0..3 {
            let values = (by..by + 8).flat_map(|y| {
                (bx..bx + 8).map(move |x| pixels[((y * SIDE + x) * 4) as usize + channel])
            });
            let (low, high) = values.fold((u8::MAX, 0), |(low, high), v| (low.min(v), high.max(v)));
            *spread = (*spread).max(high - low);
        }
    }
    spreads
}

/// A full-screen face two texels on a side: each water texel cell covers an
/// 8 × 8 pixel block, far below the fade threshold, so every cell moves.
fn coarse_quad() -> EnvironmentVolumeRecord {
    let mut volume = full_screen_fluid();
    for vertex in &mut volume.surface_faces[0].vertices {
        vertex.uv = vertex.uv.map(|texel| texel / 32.0);
    }
    volume
}

// Acceptance rows 26 and 27 on an adapter. A renderer no manifest has spoken
// to (frame capture's) draws pixel style: every pixel of one water texel cell
// reads one reflection, so each cell's block holds one value. Smooth varies
// per pixel inside the same blocks. The style uploads on change through the
// scene uniform, and switching back restores the pixel frame.
#[test]
fn pixel_style_reflects_once_per_water_texel_cell_and_smooth_per_pixel() {
    use postretro_render_cpu::fluid_surface::reflection_style::{
        FLUID_REFLECTION_STYLE_PIXEL, FLUID_REFLECTION_STYLE_SMOOTH, FluidReflectionStyle,
    };
    let Some(mut renderer) = renderer() else {
        return;
    };
    assert_eq!(
        renderer.fluid_reflection_style(),
        FluidReflectionStyle::Pixel
    );
    let uploaded = |renderer: &Renderer| {
        renderer
            .full()
            .fluid_surface
            .scene_inputs()
            .uploaded()
            .reflection_style
    };
    assert_eq!(uploaded(&renderer), FLUID_REFLECTION_STYLE_PIXEL);

    let pool = coarse_quad();
    let dimension = 16;
    let probes = ReflectionProbesSection {
        tile_dimension: dimension,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_RGBA16F,
        probes: vec![ReflectionProbeRecord {
            source_entity: 0,
            box_min: [-8.0; 3],
            box_max: [8.0; 3],
            capture_point: [0.0, 0.0, 0.0],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        }],
        bindings: vec![ReflectionProbeBinding {
            volume: 0,
            face: 0,
            probe: 0,
        }],
        tiles: gradient_rgba16f_tile(dimension),
    };
    // A mirror (reflectivity 50) at zero opacity: the output is the
    // reflection alone.
    let look = FluidSurfaceLook {
        surface_reflectivity: 50.0,
        has_bound_face: true,
        ..FluidSurfaceLook::surface_only(0.0)
    };
    let grazing = Vec3::new(0.0, 3.0, -0.5);
    renderer.install_fluid_surfaces(std::slice::from_ref(&pool), Some(&probes));
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let pixel = capture_from(&mut renderer, grazing);
    let pixel_spreads = cell_block_spreads(&pixel);

    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let smooth = capture_from(&mut renderer, grazing);
    assert_eq!(uploaded(&renderer), FLUID_REFLECTION_STYLE_SMOOTH);
    let smooth_spreads = cell_block_spreads(&smooth);

    renderer.set_fluid_reflection_style(FluidReflectionStyle::Pixel);
    renderer.set_fluid_surface_draws(&[look_draw(0, look)]);
    let restored = capture_from(&mut renderer, grazing);
    assert_eq!(uploaded(&renderer), FLUID_REFLECTION_STYLE_PIXEL);
    eprintln!(
        "[FluidSurfaceProof] reflection style GPU case ran: pixel cell spreads \
         {pixel_spreads:?}, smooth {smooth_spreads:?}"
    );
    assert!(
        pixel_spreads.iter().all(|&spread| spread <= 1),
        "each pixel-style cell holds one reflection: {pixel_spreads:?}"
    );
    assert!(
        smooth_spreads.iter().all(|&spread| spread >= 4),
        "smooth varies per pixel inside each cell: {smooth_spreads:?}"
    );
    assert!(
        pixel
            .iter()
            .zip(&restored)
            .all(|(a, b)| a.abs_diff(*b) <= 1),
        "switching back restores the pixel frame"
    );
    renderer.release_level_resources();
}
