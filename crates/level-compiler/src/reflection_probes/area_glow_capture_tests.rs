// Reflection capture of `light_area` glow: at the light's reference state, whatever the texture strength, inside the light's share.
// See: context/lib/resource_management.md §4.5, §5.2

use glam::Vec3;
use postretro_level_format::area_glow::{AreaGlowLight, AreaGlowSection};
use postretro_level_format::emissive::EmissiveTexture;
use postretro_level_format::reflection_probes::reflection_probe_lobe_weights;
use postretro_level_format::sh_volume::AnimationDescriptor;

use super::reference::reference_value;
use super::tests::{
    CEILING, FLOOR, LitFloorRoom, bake, descriptor, lit_floor_room, pool, tile_texel,
    uniform_block, uniform_weight_maps, write_glowing_prm,
};

/// The ceiling's `_e` texel, sRGB 188: about linear 0.5.
fn glass_texel() -> f32 {
    ((188.0f32 / 255.0 + 0.055) / 1.055).powf(2.4)
}

/// BC6H's error on these smooth values: a few percent.
fn close(actual: Vec3, expected: Vec3) {
    let error = ((actual - expected).abs() / expected.max(Vec3::splat(1e-3))).max_element();
    assert!(error < 0.03, "{actual} vs {expected}");
}

/// A room floored in `floor` whose ceiling (texture `wall`) has a grey diffuse
/// and an `_e` map, the ceiling being one area light's only emitting face;
/// `wall_strength` is the ceiling texture's baked strength.
fn glass_room(floor: [u8; 4], wall_strength: f32, light: AreaGlowLight) -> LitFloorRoom {
    let mut room = lit_floor_room(floor);
    write_glowing_prm(
        room.prm_dir.path(),
        [2; 32],
        [128, 128, 128, 255],
        [188, 188, 188, 255],
    );
    room.extras.emissive = vec![
        EmissiveTexture::DARK,
        EmissiveTexture {
            strength: wall_strength,
            glow_color: [glass_texel(); 3],
        },
    ];
    room.extras.area_glow = Some(AreaGlowSection {
        lights: vec![AreaGlowLight {
            faces: vec![CEILING as u32],
            ..light
        }],
    });
    room
}

fn area_light(color: [f32; 3], intensity: f32, compose_slot: Option<u32>) -> AreaGlowLight {
    AreaGlowLight {
        glow: 4.0,
        color,
        intensity,
        compose_slot,
        faces: Vec::new(),
    }
}

/// A brightness curve whose clamped cycle mean is exactly 0.5: a closed
/// two-key loop is smoothstep up then down.
fn half_mean_pulse(base_color: [f32; 3]) -> AnimationDescriptor {
    descriptor(base_color, vec![0.0, 1.0])
}

// Acceptance row 47 (static half): reflection capture shows an area light's
// glass at `_e` × `_glow` × `_color` whatever its texture strength, 0
// included; the walls sharing that texture glow at their strength, 0 here
// at strength 0.
#[test]
fn capture_shows_static_area_glass_at_glow_times_colour_whatever_its_strength() {
    let red = [1.0, 0.0, 0.0];
    let expected = Vec3::from(red) * glass_texel() * 4.0;
    for strength in [0.0, 1.0, 4.0] {
        let room = glass_room([0, 0, 0, 255], strength, area_light(red, 3.0, None));
        let section = bake(&room, &pool()).expect("the pool keeps its probe");
        close(tile_texel(&section, 0, Vec3::Y), expected);
    }
    // A side wall shares the texture and is no area face: dark at strength 0.
    let room = glass_room([0, 0, 0, 255], 0.0, area_light(red, 3.0, None));
    let section = bake(&room, &pool()).unwrap();
    assert!(tile_texel(&section, 0, Vec3::X).length() < 0.02);
}

// Acceptance rows 47 and 48 (research.md O2): an animated area light's glass
// captures at `_e` × `_glow` × `_color` × its curve's cycle mean (0.5),
// matching its live glow at that mean, never × `light`: compiling at L and
// at 2L leaves the glass unchanged and doubles the reflected room. The
// capture is the same whether or not the light starts inactive.
#[test]
fn capture_shows_animated_area_glass_at_its_reference_and_never_times_light() {
    let red = [1.0, 0.0, 0.0];
    let capture_at = |intensity: f32, start_active: u32| {
        let mut room = glass_room(
            [255, 255, 255, 255],
            0.0,
            area_light(red, intensity, Some(0)),
        );
        let mut light = half_mean_pulse((Vec3::from(red) * intensity).to_array());
        light.start_active = start_active;
        room.extras.sh.animation_descriptors = vec![light];
        // The room the light lights: the white floor, its static light off.
        room.lightmap.blocks[0] = uniform_block(8, 8, 0.0);
        uniform_weight_maps(&mut room, &[(FLOOR, &[(0, 0.5)])]);
        bake(&room, &pool()).expect("the pool keeps its probe")
    };
    let mean = reference_value(&half_mean_pulse(red));
    assert!((mean[0] - 0.5).abs() < 1e-6, "{mean:?}");

    let at_l = capture_at(1.0, 1);
    let glass = tile_texel(&at_l, 0, Vec3::Y);
    close(glass, Vec3::from(red) * glass_texel() * 4.0 * 0.5);
    // The live glow at the cycle mean (render-cpu's mirror of the shader).
    let live: Vec3 = (0..64)
        .map(|step| {
            let baked = half_mean_pulse(red);
            Vec3::from(postretro_render_cpu::emissive_volume::area_glow_factor(
                postretro_render_cpu::emissive_volume::area_glow_entry(&area_light(
                    red,
                    1.0,
                    Some(0),
                )),
                Some(
                    postretro_render_cpu::sh_compose::AnimatedLightScaleDescriptor {
                        period: baked.period,
                        phase: baked.phase,
                        base_color: baked.base_color,
                        brightness: &baked.brightness,
                        color: &[],
                        is_active: true,
                    },
                ),
                (step as f32 + 0.5) / 64.0,
            ))
        })
        .sum::<Vec3>()
        / 64.0;
    close(live * glass_texel(), glass);

    let at_2l = capture_at(2.0, 1);
    let glass_2l = tile_texel(&at_2l, 0, Vec3::Y);
    assert_eq!(glass_2l, glass, "the glass ignores `light`");
    let room_l = tile_texel(&at_l, 0, Vec3::NEG_Y);
    let room_2l = tile_texel(&at_2l, 0, Vec3::NEG_Y);
    assert!(room_l.x > 0.05, "the floor is lit: {room_l}");
    close(room_2l, room_l * 2.0);
    assert_eq!(
        Vec3::from(at_2l.probes[0].lights[0].reference),
        Vec3::from(at_l.probes[0].lights[0].reference) * 2.0
    );

    // Starting inactive captures the same bytes: the reference ignores it.
    assert_eq!(capture_at(1.0, 0).to_bytes(), at_l.to_bytes());
}

// Acceptance row 47 (research.md O3): in a room lit by a `_start_inactive`
// area light alone, its glow and its light share one share entry, so the
// reflected glass and the reflected room are dark until a script lights it
// and then brighten together, on the same frame: the runtime scales both by
// one ratio.
#[test]
fn a_start_inactive_area_lights_glass_and_room_share_one_ratio() {
    let red = [1.0, 0.0, 0.0];
    let mut room = glass_room([0, 0, 0, 255], 0.0, area_light(red, 1.0, Some(0)));
    let mut light = descriptor(red, Vec::new());
    light.start_active = 0;
    room.extras.sh.animation_descriptors = vec![light.clone()];
    // The light also lights the grey ceiling around the glow: the room.
    uniform_weight_maps(&mut room, &[(CEILING, &[(0, 0.5)])]);
    let section = bake(&room, &pool()).expect("the pool keeps its probe");
    let entry = section.probes[0]
        .lights
        .iter()
        .find(|entry| entry.roster_index == 0)
        .expect("the light holds a share entry");
    let lobes = reflection_probe_lobe_weights(Vec3::Y.to_array());
    let share: f32 = (0..lobes.len())
        .map(|lobe| entry.shares[lobe][0] * lobes[lobe])
        .sum();
    assert!(
        (share - 1.0).abs() < 1e-3,
        "glass and room are all the light above the water: {entry:?}"
    );
    // The runtime ratio, `1 + share × (current / reference − 1)`.
    let reference = entry.reference[0];
    let ratio = |current: f32| 1.0 + share * (current / reference - 1.0);
    assert!(ratio(0.0).abs() < 1e-3, "dark while inactive");
    assert!((ratio(reference) - 1.0).abs() < 1e-6, "lit at reference");
    // Glass and room are both in that captured texel: they scale as one.
    let up = tile_texel(&section, 0, Vec3::Y);
    assert!(up.x > 0.5, "{up}");
}
