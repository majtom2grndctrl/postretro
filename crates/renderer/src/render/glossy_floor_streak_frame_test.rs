// GPU proof of where the floor layer reads a hit: at the march's last sample in
// front of the depth buffer, not at its stop. A bright bar standing half a
// metre in front of a dark wall, and a bright strip 2 cm proud of the wall,
// over a glossy floor, at High in a 1080-row scene (a 6-pixel stride). Read at
// the stop, rays passing within the march's thickness behind the standing bar
// read the bar, so its reflection streaks along the ray over the wall it
// hides; read one stride back, it spills nothing, either reflection falls
// short by at most a stride, and the flush strip spills less. The march's
// confidence is the same bits either way.
// See: context/lib/testing_guide.md · context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

use glam::{UVec2, Vec2, Vec3, Vec3Swizzles, Vec4Swizzles};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;

use super::GlossyFloorMarchPipelines;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{View, view_from};
use crate::render::fluid_reflection_sums_frame_test_fixtures::frame_at;
use crate::render::glossy_floor_frame_test_fixtures::{
    FloorLayer, install_faces, read_floor_layer,
};
use crate::render::glossy_overlay_frame_test_fixtures::{
    FixtureFace, FloorMaterial, floor_probe, set_bucket_material,
};

/// The scene: 1080 rows, so High's stride is 1080 / 180 = 6 pixels.
const SCENE: u32 = 1080;
/// The floor at y = 0 over x ∈ [-3, 3], z ∈ [1, 9]; the wall at z = 9, 3 m
/// tall.
const WALL_Z: f32 = 9.0;
/// Both bars' height band: 15 cm, about 21 scene pixels from the eye.
const BAR_Y: [f32; 2] = [1.0, 1.15];
/// The standing bar: half a metre in front of the wall, inside High's 1 m
/// thickness, so a ray passing behind it stops on it.
const STANDING: Bar = Bar {
    z: 8.5,
    x: [-2.6, -0.4],
};
/// The flush strip: 2 cm in front of the wall.
const FLUSH: Bar = Bar {
    z: 8.98,
    x: [0.4, 2.6],
};
/// Metres kept clear of a bar's ends, where its sides would enter the count.
const END_MARGIN: f32 = 0.25;

/// A bright bar facing the eye (−Z): its plane and its x extent.
#[derive(Clone, Copy)]
struct Bar {
    z: f32,
    x: [f32; 2],
}

/// The level's faces, buckets ascending: 0 the glossy floor (bound to the
/// probe), 1 the dark wall, 2 both bars.
fn faces() -> Vec<FixtureFace> {
    let up = postretro_level_format::octahedral::encode(0.0, 1.0, 0.0);
    let toward_eye = postretro_level_format::octahedral::encode(0.0, 0.0, -1.0);
    let upright = |x: [f32; 2], y: [f32; 2], z: f32, bucket| FixtureFace {
        corners: [
            [x[0], y[0], z],
            [x[1], y[0], z],
            [x[1], y[1], z],
            [x[0], y[1], z],
        ],
        normal_oct: toward_eye,
        bucket,
    };
    vec![
        FixtureFace {
            corners: [
                [-3.0, 0.0, 1.0],
                [3.0, 0.0, 1.0],
                [3.0, 0.0, WALL_Z],
                [-3.0, 0.0, WALL_Z],
            ],
            normal_oct: up,
            bucket: 0,
        },
        upright([-3.0, 3.0], [0.0, 3.0], WALL_Z, 1),
        upright(STANDING.x, BAR_Y, STANDING.z, 2),
        upright(FLUSH.x, BAR_Y, FLUSH.z, 2),
    ]
}

/// The eyes: 1.50 m to 1.60 m above the floor at its near end, looking 7°
/// down the room, so the bars stand above the screen's centre and their
/// reflections below. Each height moves every ray's sample grid against the
/// surfaces it crosses, so together they cover the stride's phases.
const EYE_HEIGHTS: [f32; 6] = [1.50, 1.52, 1.54, 1.56, 1.58, 1.60];

fn view(height: f32) -> View {
    view_from(
        Vec3::new(0.0, height, 2.0),
        Vec3::new(0.0, height - 0.9, WALL_Z),
    )
}

/// The production march in smooth style over the fixture's plain materials,
/// with its read moved: at the stop sample (the read before) or at
/// `glossy_floor_read_pixel`, through the production blur. The sharp entries
/// read the chain's level 0, which shows the read point texel for texel, and
/// write the hit's red channel beside where the march's own ray crosses each
/// bar's plane (the neutral normal byte tilts it off the CPU's mirror):
/// (X.r, y at the standing bar, y at the flush strip, c).
const VARIANTS: &str = r#"
fn test_streak_crossing(origin: vec3<f32>, ray: vec3<f32>, z: f32) -> f32 {
    return (origin + ray * ((z - origin.z) / ray.z)).y;
}

fn test_streak_march(in: GlossyVertexOutput, back: bool, sharp: bool) -> vec4<f32> {
    let derivatives = glossy_floor_derivatives(in);
    let depth = glossy_overlay_depth(in, derivatives, false);
    let exponent = glossy_floor_exponent(depth.uv, derivatives.ddx, derivatives.ddy);
    let surface = glossy_overlay_surface(in, derivatives, depth, false, false);
    if !fluid_reflection_seen_from_above(surface.outward, surface.view)
        || !fluid_reflection_mask_on(uniforms.light_term_mask) {
        discard;
    }
    let ray = fluid_reflection_above_hemisphere(fluid_reflection_mirror(surface.view,
        surface.normal), fluid_reflection_hemisphere_axis(surface.outward));
    let extent = vec2<f32>(textureDimensions(scene_depth));
    let focal = glossy_floor_focal(extent);
    let origin = glossy_floor_origin(surface.origin, surface.outward, focal);
    let march = screen_march(origin, ray, uniforms.view_proj, scene_depth,
        floor_march.stride, floor_march.step_budget, floor_march.thickness);
    let read = select(march.pixel, glossy_floor_read_pixel(origin, march.pixel, extent), back);
    let colour = glossy_floor_hit_colour(read, exponent, floor_march.term_blur,
        glossy_floor_hit(origin, ray, read, extent), extent, focal);
    if sharp {
        let x = glossy_floor_blurred_hit(read / extent, 0.0).r;
        let crossings = vec2<f32>(test_streak_crossing(origin, ray, STANDING_Z),
            test_streak_crossing(origin, ray, FLUSH_Z));
        return select(vec4<f32>(0.0), vec4<f32>(x, crossings, march.confidence),
            march.confidence > 0.0);
    }
    let premultiplied = vec4<f32>(colour * march.confidence, march.confidence);
    return select(vec4<f32>(0.0), premultiplied, march.confidence > 0.0);
}

@fragment
fn fs_test_stop(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return test_streak_march(in, false, false);
}

@fragment
fn fs_test_stop_sharp(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return test_streak_march(in, false, true);
}

@fragment
fn fs_test_back_sharp(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return test_streak_march(in, true, true);
}
"#;

/// [`VARIANTS`] with the bars' planes declared.
fn variants() -> String {
    format!(
        "const STANDING_Z: f32 = {:?};\nconst FLUSH_Z: f32 = {:?};\n{VARIANTS}",
        STANDING.z, FLUSH.z
    )
}

/// A renderer at [`SCENE`]² with bloom off, or `None` without an adapter.
fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(SCENE, SCENE) {
        Ok(mut renderer) => {
            renderer.full_mut().bloom.set_enabled(false);
            Some(renderer)
        }
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[GlossyFloorStreak] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// One frame's floor layer through `view`, by the variant `entry` or, with
/// `None`, the production entries.
fn floor_layer(renderer: &mut Renderer, view: View, entry: Option<&str>) -> FloorLayer {
    let production = entry.map(|entry| {
        let variant = renderer.full().glossy_overlay.floor().march_variant(
            &renderer.device,
            &variants(),
            entry,
        );
        renderer
            .full_mut()
            .glossy_overlay
            .floor_mut()
            .swap_march(GlossyFloorMarchPipelines::every(variant))
    });
    frame_at(renderer, view, 0.0);
    if let Some(production) = production {
        renderer
            .full_mut()
            .glossy_overlay
            .floor_mut()
            .swap_march(production);
    }
    read_floor_layer(renderer).expect("the floor tier runs the floor layer")
}

/// Where target texel `texel`'s reflected ray crosses `bar`'s plane, when
/// its scene position shows the floor: the crossing's x and y. The ray starts
/// at the floor point under the texel's centre, as smooth style's fragment
/// does, and mirrors the view ray about the floor's +Y normal; the sharp
/// entries write the march's own ray's y, which the neutral normal byte
/// tilts by about a centimetre at the bars.
fn crossing(view: View, texel: UVec2, extent: UVec2, bar: Bar) -> Option<Vec2> {
    let ndc =
        (texel.as_vec2() + 0.5) / extent.as_vec2() * Vec2::new(2.0, -2.0) + Vec2::new(-1.0, 1.0);
    let inverse = view.view_proj.inverse();
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    let dir = (far - near).normalize();
    if dir.y >= 0.0 {
        return None;
    }
    let floor = near + dir * (-near.y / dir.y);
    if !(floor.x.abs() < 3.0 && floor.z > 1.0 && floor.z < STANDING.z) {
        return None;
    }
    let mirrored = Vec3::new(dir.x, -dir.y, dir.z);
    let t = (bar.z - floor.z) / mirrored.z;
    (t > 0.0).then(|| (floor + mirrored * t).xy())
}

/// How a read placement reflects one bar, over the texels whose rays cross
/// the bar's plane inside its x extent less [`END_MARGIN`].
#[derive(Debug, Default)]
struct Streak {
    /// Texels whose ray crosses the bar.
    on_bar: usize,
    /// Texels reading the bar bright whose ray passes outside it, and the
    /// farthest such crossing from the bar's band, metres.
    spilled: usize,
    spill_reach: f32,
    /// Texels whose ray crosses the bar that read it dark, and the farthest
    /// such crossing inside the band from its nearer edge, metres.
    missed: usize,
    miss_reach: f32,
}

/// Add `layer`'s reflection of `bar` (its crossings in `channel`) through
/// `view` to `streak`, the bar reading bright where X exceeds `threshold`.
fn add_streak(
    streak: &mut Streak,
    view: View,
    layer: &FloorLayer,
    (bar, channel): (Bar, usize),
    threshold: f32,
) {
    for y in 0..layer.extent.y {
        for x in 0..layer.extent.x {
            let texel = UVec2::new(x, y);
            let Some(at) = crossing(view, texel, layer.extent, bar) else {
                continue;
            };
            if !(at.x > bar.x[0] + END_MARGIN && at.x < bar.x[1] - END_MARGIN) {
                continue;
            }
            let stored = layer.at(texel);
            if stored.w <= 0.0 {
                continue;
            }
            let bright = stored.x > threshold;
            let y = stored[channel];
            let outside = (BAR_Y[0] - y).max(y - BAR_Y[1]);
            if outside <= 0.0 {
                streak.on_bar += 1;
                if !bright {
                    streak.missed += 1;
                    streak.miss_reach = streak.miss_reach.max(-outside);
                }
            } else if bright {
                streak.spilled += 1;
                streak.spill_reach = streak.spill_reach.max(outside);
            }
        }
    }
}

// At High, smooth style, over six eye heights, read sharp at the chain's
// level 0 and measured against the march's own ray: at the stop, the standing
// bar's reflection spills over the wall it hides by more than its own area;
// one stride back it spills nothing and falls short by at most a stride, and
// the flush strip spills less than at the stop. Through the production blur,
// the production read keeps the stop read's confidence bit for bit.
#[test]
fn the_floor_layer_reads_a_hit_at_the_marchs_last_sample_in_front() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    install_faces(&mut renderer, &faces(), &floor_probe(true));
    set_bucket_material(&mut renderer, 0, FloorMaterial::WHITE_SHARP, true);
    let dark = FloorMaterial {
        albedo: [16, 16, 16, 255],
        slot: [0, 0, 0, 255],
        glow: None,
    };
    set_bucket_material(&mut renderer, 1, dark, false);
    let bright = FloorMaterial {
        glow: Some(([255; 4], 8.0)),
        ..dark
    };
    set_bucket_material(&mut renderer, 2, bright, false);
    renderer.set_ambient_floor(0.5);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);

    // Sharp reads at every eye height; the threshold is half the brightest
    // reflected bar.
    let layers: Vec<(View, FloorLayer, FloorLayer)> = EYE_HEIGHTS
        .iter()
        .map(|&height| {
            let view = view(height);
            (
                view,
                floor_layer(&mut renderer, view, Some("fs_test_stop_sharp")),
                floor_layer(&mut renderer, view, Some("fs_test_back_sharp")),
            )
        })
        .collect();
    let peak = layers
        .iter()
        .flat_map(|(_, stop, _)| &stop.texels)
        .map(|texel| texel.x)
        .fold(0.0_f32, f32::max);
    assert!(peak > 1.0, "the bars reflect bright ({peak})");
    let threshold = 0.5 * peak;
    // One stride (6 scene pixels) on the bars' planes at their view depth
    // (about 6.5 m) under a 60° field of view: 6 × 6.5 / 935 ≈ 4.2 cm.
    let stride_at_bar = 6.0 * 6.5 / (0.5 * SCENE as f32 / 30.0_f32.to_radians().tan());
    let measure = |name: &str, bar: (Bar, usize)| {
        let (mut before, mut after) = (Streak::default(), Streak::default());
        for (view, stop, back) in &layers {
            add_streak(&mut before, *view, stop, bar, threshold);
            add_streak(&mut after, *view, back, bar, threshold);
        }
        eprintln!(
            "[GlossyFloorStreak] {name}: stop {before:?}; one stride back {after:?}; \
             stride ≈ {stride_at_bar:.3} m"
        );
        assert!(
            before.on_bar > 1000 && after.on_bar == before.on_bar,
            "{name}: the bar's reflection covers many texels ({before:?})"
        );
        (before, after)
    };
    // On Metal: at the stop, the standing bar's reflection spills over about
    // 1.6 times its own texels, reaching 25 cm (six strides) past it; one
    // stride back nothing spills, and what falls short reaches under a
    // stride. The flush strip's spill drops from 1270 texels reaching 5.1 cm
    // to 711 reaching 2.8 cm, its shortfall from 3.9 to 4.2 cm.
    let (before, after) = measure("standing bar", (STANDING, 1));
    assert!(
        before.spilled > before.on_bar && before.spill_reach > 3.0 * stride_at_bar,
        "the stop read streaks the standing bar over the wall behind it: {before:?}"
    );
    assert!(
        after.spilled * 100 < before.spilled && after.spill_reach < 0.25 * stride_at_bar,
        "one stride back, the standing bar's reflection does not spill: {after:?}"
    );
    assert!(
        after.miss_reach < 1.25 * stride_at_bar,
        "one stride back falls short of the standing bar by at most a stride: {after:?}"
    );
    let (before, after) = measure("flush strip", (FLUSH, 2));
    assert!(
        after.spilled < before.spilled && after.spill_reach < before.spill_reach,
        "one stride back spills the flush strip less than the stop read: \
         stop {before:?}, back {after:?}"
    );
    assert!(
        after.miss_reach < 1.25 * stride_at_bar,
        "one stride back falls short of the flush strip by at most a stride: {after:?}"
    );

    // Through the production blur: confidence is the stop read's, bit for
    // bit, and only where the hit is read moved.
    let view = view(EYE_HEIGHTS[0]);
    let stop = floor_layer(&mut renderer, view, Some("fs_test_stop"));
    let production = floor_layer(&mut renderer, view, None);
    assert!(
        stop.texels
            .iter()
            .zip(&production.texels)
            .all(|(a, b)| a.w.to_bits() == b.w.to_bits()),
        "the march's confidence is the same bits wherever the hit is read"
    );
    let moved = stop
        .texels
        .iter()
        .zip(&production.texels)
        .filter(|(a, b)| a.xyz() != b.xyz())
        .count();
    assert!(
        moved > 0,
        "the production entry reads the hit one stride back"
    );
}
