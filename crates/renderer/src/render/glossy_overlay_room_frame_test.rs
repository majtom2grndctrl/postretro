//! Adapter-backed gates for the glossy overlay on every orientation: a floor,
//! a wall and a down-facing ceiling read their probes through the sums within
//! 1/255 of the CPU reference, each in its own hemisphere; a ceiling takes its
//! floor panel's share on the −Y lobe; a skylight's reflected glass and room
//! scale together when a script switches it, at every tier; the glass blurs
//! by each texel's exponent with no highlight; a glossy `light_area` glass
//! keeps its glow at W = 1; and scripted and animated glow pass through the
//! overlay unchanged.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Live lighting), §7.3 (Area glow)

use glam::{UVec2, Vec3};
use postretro_level_format::area_glow::{AREA_GLOW_FIRST_ENTRY, AreaGlowLight};
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_format::sh_volume::AnimationDescriptor;
use postretro_render_cpu::emissive_volume::{
    EMISSIVE_TABLE_BYTES, EMISSIVE_TABLE_ENTRY_BYTES, EmissiveTableUpload, area_glow_entry,
    area_glow_factor,
};
use postretro_render_cpu::fluid_surface::reflection::reflection_ratio_from_sums;
use postretro_render_cpu::fluid_surface::reflection_hemisphere::{
    FLUID_REFLECTION_UP, reflection_level,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::sh_compose::AnimatedLightScaleDescriptor;

use super::Renderer;
use super::fluid_reflection_frame_test_fixtures::View;
use super::fluid_reflection_sums_frame_test_fixtures::descriptor_record;
use super::glossy_overlay_frame_test_fixtures::{
    GlossyFrame, half_step, renderer, with_probe_term_alone, with_variant,
};
use super::glossy_overlay_room_frame_test_fixtures::{
    Face, Room, RoomMaterial, RoomReference, face_pixels, flat, room_floor_view, room_frame,
    room_view, stored,
};

/// Pixels this close (metres) to a face edge are left out.
const MARGIN: f32 = 0.08;
/// Row 33's bound: 1/255 of a channel, plus the scene colour's own f16 step.
fn bound(value: f32) -> f32 {
    1.0 / 255.0 + half_step(value)
}

/// Production at `time` and the zero-adding variant (forward's colour and
/// D_static) at the same pose and clock. The room's floor reflects its walls
/// through the floor layer at the tiers that run it; these proofs hold the
/// probe term, so production composites the probe term alone.
fn frames(renderer: &mut Renderer, view: View, time: f32) -> (GlossyFrame, GlossyFrame) {
    let production = with_probe_term_alone(renderer, |renderer| room_frame(renderer, view, time));
    let forward = with_variant(renderer, "fs_test_zero", |renderer| {
        room_frame(renderer, view, time)
    });
    (production, forward)
}

/// Rewrite roster light `slot` as a script's light change arrives, keeping
/// `roster` as the GPU will hold it after the frame's flush.
fn force(
    renderer: &mut Renderer,
    roster: &mut [AnimationDescriptor],
    slot: usize,
    light: AnimationDescriptor,
) {
    renderer.write_animated_compose_descriptor(slot as u32, &descriptor_record(&light));
    roster[slot] = light;
}

/// Assert every pixel of `face` matches the reference within row 33's bound,
/// returning how many it checked and the largest gap.
fn assert_face_matches(
    reference: &RoomReference<'_>,
    production: &GlossyFrame,
    forward: &GlossyFrame,
    view: View,
    face: Face,
    case: &str,
) -> (usize, f32) {
    let mut worst: f32 = 0.0;
    let pixels = face_pixels(view, face, MARGIN);
    for &(pixel, point) in &pixels {
        let want = forward.colour(pixel)
            + reference
                .addition(face, point, forward.static_diffuse(pixel))
                .added;
        let got = production.colour(pixel);
        for c in 0..3 {
            let gap = (got[c] - want[c]).abs();
            worst = worst.max(gap);
            assert!(
                gap <= bound(want[c]),
                "{case}: {face:?} at {pixel}: channel {c} is {} against {}",
                got[c],
                want[c]
            );
        }
    }
    assert!(
        pixels.len() > 150,
        "{case}: {face:?} shows {} pixels",
        pixels.len()
    );
    (pixels.len(), worst)
}

fn uniform(colour: Vec3) -> fn(Vec3, u32) -> Vec3 {
    // Exact in f16; one colour per face, distinct.
    match (colour.x * 8.0) as u32 {
        2 => |_, _| Vec3::new(0.25, 0.5, 0.125),
        4 => |_, _| Vec3::new(0.5, 0.25, 0.375),
        _ => |_, _| Vec3::new(0.375, 0.125, 0.5),
    }
}

const REFERENCE: [f32; 3] = [0.8, 0.6, 0.4];

// Acceptance row 33 (and the GPU half of pin P5 per orientation): a glossy
// mirror (W = 1) floor, wall and down-facing ceiling, each bound to a probe storing one
// light with a different share on every lobe, match the GPU-free reference
// within 1/255 per channel through the sums, at the light's reference, forced
// to twice it and forced off, on the frame it is forced. Each face's lookups
// keep its own hemisphere: about the horizon instead, the wall and ceiling
// miss the reference.
#[test]
fn floor_wall_and_ceiling_live_lighting_match_the_reference_through_the_sums() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut roster = vec![flat(REFERENCE, true)];
    let room = Room {
        roster: roster.clone(),
        lights: [
            vec![stored(0, REFERENCE, [0.6, 0.2, 0.1, 0.3, 0.4, 0.05])],
            vec![stored(0, REFERENCE, [0.3, 0.5, 0.2, 0.1, 0.25, 0.6])],
            vec![stored(0, REFERENCE, [0.05, 0.2, 0.3, 0.1, 0.4, 0.8])],
        ],
        ..Room::new([
            uniform(Vec3::splat(0.25)),
            uniform(Vec3::splat(0.5)),
            uniform(Vec3::splat(0.375)),
        ])
    };
    let material = RoomMaterial::WHITE_MIRROR;
    room.install(&mut renderer, &material);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let view = room_view();
    let (at_reference, _) = frames(&mut renderer, view, 0.0);
    let twice = REFERENCE.map(|c| c * 2.0);
    for (case, light) in [
        ("at reference", flat(REFERENCE, true)),
        ("twice", flat(twice, true)),
        ("off", flat(REFERENCE, false)),
    ] {
        force(&mut renderer, &mut roster, 0, light);
        let (production, forward) = frames(&mut renderer, view, 0.0);
        let reference = RoomReference::new(&room, view, &roster, 0.0, material);
        for face in Face::ALL {
            let (checked, worst) =
                assert_face_matches(&reference, &production, &forward, view, face, case);
            eprintln!("[row 33] {case}: {face:?} {checked} pixels, worst {worst}");
            if case != "at reference" {
                // The forced light reaches every orientation on its frame.
                let moved = face_pixels(view, face, MARGIN)
                    .iter()
                    .filter(|(pixel, _)| {
                        (production.colour(*pixel) - at_reference.colour(*pixel))
                            .abs()
                            .max_element()
                            > 0.01
                    })
                    .count();
                assert!(moved > 100, "{case}: {face:?} moved on {moved} pixels");
            }
        }
        if case == "twice" {
            // About the horizon (the clamp before the hemisphere), the wall's
            // and ceiling's lookups lose their lower half and miss.
            for face in [Face::Wall, Face::Ceiling] {
                let missed = face_pixels(view, face, MARGIN)
                    .iter()
                    .filter(|&&(pixel, point)| {
                        let up = reference.addition_about(
                            face,
                            point,
                            (0, 0),
                            Vec3::ONE,
                            forward.static_diffuse(pixel),
                            FLUID_REFLECTION_UP,
                        );
                        let want = forward.colour(pixel) + up.added;
                        (production.colour(pixel) - want).abs().max_element() > 2.0 / 255.0
                    })
                    .count();
                assert!(
                    missed > 100,
                    "{face:?}: the horizon clamp misses {missed} pixels"
                );
            }
        }
    }
}

/// The ceiling's probe colour: the floor's `light_area` panel straight
/// below (bright), the lit floor around it, the walls above.
fn panel_below(direction: Vec3, _mip: u32) -> Vec3 {
    let d = direction.normalize();
    if d.y < -0.95 {
        Vec3::new(2.0, 1.75, 1.5)
    } else if d.y < 0.0 {
        Vec3::new(0.25, 0.25, 0.25)
    } else {
        Vec3::new(0.125, 0.125, 0.125)
    }
}

const PANEL: [f32; 3] = [1.0, 0.9, 0.8];

// Acceptance row 19 (GPU half): a glossy down-facing ceiling over a floor
// `light_area` panel, whose probe stores the panel's light on the −Y lobe
// alone, reflects the panel and its lit floor; pulsing the light brightens
// both on the pulse frame, each by the reference's ratio. About the horizon
// the ceiling's lookups would carry no −Y weight and nothing would move.
#[test]
fn a_ceiling_takes_its_floor_panels_share_on_the_down_lobe_and_a_pulse_moves_both() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut roster = vec![flat(PANEL, true)];
    let room = Room {
        roster: roster.clone(),
        lights: [
            Vec::new(),
            Vec::new(),
            vec![stored(0, PANEL, [0.0, 0.0, 0.0, 0.0, 0.0, 1.0])],
        ],
        ..Room::new([panel_below, panel_below, panel_below])
    };
    let material = RoomMaterial::WHITE_SHARP;
    room.install(&mut renderer, &material);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    // Looking up at the ceiling from near the floor.
    let view = super::fluid_reflection_frame_test_fixtures::view_from(
        Vec3::new(0.0, 0.6, 2.0),
        Vec3::new(0.0, 3.0, 6.0),
    );
    let (steady, _) = frames(&mut renderer, view, 0.0);
    force(
        &mut renderer,
        &mut roster,
        0,
        flat(PANEL.map(|c| c * 1.5), true),
    );
    let (pulse, forward) = frames(&mut renderer, view, 0.0);
    let reference = RoomReference::new(&room, view, &roster, 0.0, material);
    assert_face_matches(&reference, &pulse, &forward, view, Face::Ceiling, "pulse");
    let (mut panel, mut floor) = (0, 0);
    for (pixel, point) in face_pixels(view, Face::Ceiling, MARGIN) {
        let lookup = reference.lookup(Face::Ceiling, point).normalize();
        let rose = pulse.colour(pixel) - steady.colour(pixel);
        let shows_panel = lookup.y < -0.97;
        let shows_floor = lookup.y < -0.3 && lookup.y > -0.9;
        if !(shows_panel || shows_floor) {
            continue;
        }
        assert!(
            rose.min_element() > 0.0,
            "{pixel}: the reflection did not brighten on the pulse frame: {rose}"
        );
        if shows_panel {
            panel += 1;
        } else {
            floor += 1;
        }
    }
    assert!(
        panel > 10 && floor > 50,
        "panel {panel}, floor {floor} pixels"
    );
    // About the horizon a downward lookup levels onto the horizon, where the
    // −Y lobe weighs nothing: the pulse would move nothing.
    let levelled = reflection_ratio_from_sums(
        &room.cpu_sums(&roster, 0.0)[Face::Ceiling.index()],
        reflection_level(Vec3::new(0.2, -0.9, 0.3), FLUID_REFLECTION_UP),
    );
    assert_eq!(
        levelled,
        Vec3::ONE,
        "about the horizon the −Y share reads nothing"
    );
}

/// The floor's probe colour under a skylight: the glass straight above,
/// the room elsewhere. Each further mip widens the glass's cap, as the
/// prefilter blurs it.
fn skylight(direction: Vec3, mip: u32) -> Vec3 {
    let d = direction.normalize();
    let edge = [0.97, 0.93, 0.85, 0.7, 0.5][mip.min(4) as usize];
    let glass = ((d.y - edge) / (1.0 - edge)).clamp(0.0, 1.0);
    Vec3::new(0.375, 0.25, 0.125) + Vec3::new(3.0, 3.0, 2.75) * glass
}

const SKY: [f32; 3] = [0.9, 0.9, 1.0];

// Acceptance row 21: a script turning a skylight off scales its reflected
// glass and the reflected room by one live-lighting ratio on that frame, at
// every tier that draws the probe term. The floor's probe is lit by that
// light alone (its share 1 on every lobe): off, glass and room both go dark;
// at half, both halve; on again, both return.
#[test]
fn a_scripted_skylight_scales_its_glass_and_the_room_together_on_the_frame_at_every_tier() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut roster = vec![flat(SKY, true)];
    let room = Room {
        roster: roster.clone(),
        lights: [vec![stored(0, SKY, [1.0; 6])], Vec::new(), Vec::new()],
        ..Room::new([skylight, skylight, skylight])
    };
    let material = RoomMaterial::WHITE_SHARP;
    room.install(&mut renderer, &material);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let view = room_floor_view();
    let probe = RoomReference::new(&room, view, &roster, 0.0, material);
    let floor: Vec<(UVec2, glam::Vec3, bool)> = face_pixels(view, Face::Floor, MARGIN)
        .into_iter()
        .map(|(pixel, point)| {
            let glass = probe.lookup(Face::Floor, point).normalize().y > 0.975;
            (pixel, point, glass)
        })
        .collect();
    assert!(
        floor.iter().filter(|p| p.2).count() > 10,
        "the glass reflects"
    );
    assert!(
        floor.iter().filter(|p| !p.2).count() > 100,
        "the room reflects"
    );
    use FluidReflectionQuality::*;
    for tier in [Low, Medium, High, VeryHigh, Ultra] {
        renderer.set_reflection_quality(tier);
        force(&mut renderer, &mut roster, 0, flat(SKY, true));
        let (on, _) = frames(&mut renderer, view, 0.0);
        for (case, light, ratio) in [
            ("off", flat(SKY, false), 0.0),
            ("half", flat(SKY.map(|c| c * 0.5), true), 0.5),
            ("on again", flat(SKY, true), 1.0),
        ] {
            force(&mut renderer, &mut roster, 0, light);
            let (production, forward) = frames(&mut renderer, view, 0.0);
            let reference = RoomReference::new(&room, view, &roster, 0.0, material);
            assert_face_matches(
                &reference,
                &production,
                &forward,
                view,
                Face::Floor,
                &format!("{tier:?} {case}"),
            );
            for &(pixel, point, _) in &floor {
                let full = probe.addition(Face::Floor, point, forward.static_diffuse(pixel));
                let now = reference.addition(Face::Floor, point, forward.static_diffuse(pixel));
                // One ratio for glass and room alike.
                let scaled = full.reflected * ratio;
                assert!(
                    (now.reflected - scaled).abs().max_element()
                        <= 1.0e-4 * full.reflected.max_element().max(1.0),
                    "{tier:?} {case} at {pixel}: {} against {scaled}",
                    now.reflected
                );
            }
            if case == "on again" {
                assert_eq!(
                    production.scene.colour, on.scene.colour,
                    "{tier:?}: restored"
                );
            }
        }
    }
}

// Acceptance row 20: a glossy floor under a static `light_area` skylight
// (the ceiling's glass) reflects its glass at the probe's glow, read from the
// colour mips each texel's exponent picks: a sharp texel and a soft one read
// different blurs, each the reference's. The floor draws no highlight from
// the light: forward's colour is the same at either sharpness, as an area
// light has no highlight.
#[test]
fn a_floor_under_a_skylight_reflects_its_glass_blurred_by_each_texels_exponent() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room {
        tile: 64,
        mips: 5,
        glow_ids: [0, 0, AREA_GLOW_FIRST_ENTRY as u32],
        area_lights: vec![AreaGlowLight {
            glow: 4.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            compose_slot: None,
            faces: vec![Face::Ceiling.index() as u32],
        }],
        ..Room::new([skylight, skylight, skylight])
    };
    let sharp = RoomMaterial {
        glow: Some((|_, _| [255; 4], 0.0)),
        ..RoomMaterial::WHITE_SHARP
    };
    // Smoothness byte 120: an exponent of about 20, between mips 3 and 4.
    let soft = RoomMaterial {
        slot: |_, _| [255, 127, 120, 255],
        ..sharp
    };
    room.install(&mut renderer, &sharp);
    renderer.set_fluid_reflection_style(FluidReflectionStyle::Smooth);
    let view = room_floor_view();
    let mut reflections = Vec::new();
    let mut forwards = Vec::new();
    for material in [sharp, soft] {
        super::glossy_overlay_room_frame_test_fixtures::set_room_material(&mut renderer, &material);
        let (production, forward) = frames(&mut renderer, view, 0.0);
        let reference = RoomReference::new(&room, view, &[], 0.0, material);
        assert_face_matches(
            &reference,
            &production,
            &forward,
            view,
            Face::Floor,
            "glass",
        );
        let glass = face_pixels(view, Face::Floor, MARGIN)
            .into_iter()
            .filter(|&(_, point)| reference.lookup(Face::Floor, point).normalize().y > 0.975)
            .count();
        assert!(glass > 10, "the floor reflects the glass on {glass} pixels");
        reflections.push(
            face_pixels(view, Face::Floor, MARGIN)
                .into_iter()
                .map(|(pixel, _)| production.colour(pixel) - forward.colour(pixel))
                .collect::<Vec<_>>(),
        );
        forwards.push(forward);
    }
    let blurred = reflections[0]
        .iter()
        .zip(&reflections[1])
        .filter(|(a, b)| (**a - **b).abs().max_element() > 0.02)
        .count();
    assert!(
        blurred > 50,
        "the two exponents read different blurs: {blurred}"
    );
    assert_eq!(
        forwards[0].scene.colour, forwards[1].scene.colour,
        "no highlight from the area light at either sharpness"
    );
}

// Acceptance row 22: a `light_area` glass face whose material is glossy (a
// tint-1 texel, so W = 1 at every angle) keeps its glow through the
// composite unchanged: the glow's difference to the frame, glowing minus
// dark glass, is forward's with the overlay and without it.
#[test]
fn a_glossy_light_area_glass_keeps_its_glow_at_w_one() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let room = Room {
        glow_ids: [0, 0, AREA_GLOW_FIRST_ENTRY as u32],
        area_lights: vec![AreaGlowLight {
            glow: 2.0,
            color: [1.0, 0.8, 0.6],
            intensity: 1.0,
            compose_slot: None,
            faces: vec![Face::Ceiling.index() as u32],
        }],
        ..Room::new([skylight, skylight, skylight])
    };
    let dark = RoomMaterial {
        slot: |_, _| [255, 127, 255, 254],
        glow: Some((|_, _| [0, 0, 0, 255], 0.0)),
        ..RoomMaterial::WHITE_SHARP
    };
    let glowing = RoomMaterial {
        glow: Some((|_, _| [255; 4], 0.0)),
        ..dark
    };
    room.install(&mut renderer, &dark);
    let view = room_view();
    let reference = RoomReference::new(&room, view, &[], 0.0, dark);
    let mut at = |material: &RoomMaterial| {
        super::glossy_overlay_room_frame_test_fixtures::set_room_material(&mut renderer, material);
        frames(&mut renderer, view, 0.0)
    };
    let (dark_frame, dark_forward) = at(&dark);
    let (glow_frame, glow_forward) = at(&glowing);
    let mut checked = 0;
    for (pixel, point) in face_pixels(view, Face::Ceiling, MARGIN) {
        let weight = reference
            .addition(Face::Ceiling, point, dark_forward.static_diffuse(pixel))
            .weight;
        assert!(weight > 0.999, "W {weight}");
        let with = glow_frame.colour(pixel) - dark_frame.colour(pixel);
        let without = glow_forward.colour(pixel) - dark_forward.colour(pixel);
        assert!(without.min_element() > 0.5, "the glass glows: {without}");
        for c in 0..3 {
            let slack =
                half_step(glow_frame.colour(pixel)[c]) + half_step(dark_frame.colour(pixel)[c]);
            assert!(
                (with[c] - without[c]).abs() <= slack,
                "{pixel}: the glow is {} with the overlay against {} without",
                with[c],
                without[c]
            );
        }
        checked += 1;
    }
    assert!(checked > 150, "{checked}");
}

/// A pulse whose clamped cycle runs 0 → 1 → 0 over 2 s.
fn pulse() -> AnimationDescriptor {
    AnimationDescriptor {
        period: 2.0,
        brightness: vec![0.0, 1.0],
        base_color: [1.0, 1.0, 1.0],
        start_active: 1,
        ..AnimationDescriptor::default()
    }
}

/// Write emissive volume 0's presented factor, as the volume bridge writes
/// a script's change.
fn set_volume(renderer: &mut Renderer, factor: [f32; 3]) {
    let mut table = vec![0u8; EMISSIVE_TABLE_BYTES];
    let entry = EMISSIVE_TABLE_ENTRY_BYTES;
    for (c, value) in factor.into_iter().chain([0.0]).enumerate() {
        table[entry + 4 * c..entry + 4 * c + 4].copy_from_slice(&value.to_ne_bytes());
    }
    assert!(renderer.upload_emissive_volume_table(EmissiveTableUpload {
        table: &table,
        runs: std::slice::from_ref(&(1..2)),
    }));
}

// Acceptance row 32 (the carried gap, pin P13): the difference a scripted
// `emissive_volume` face (the floor) and an animated `light_area` face (the
// ceiling) make mid-pulse, glowing minus dark, is the same with and without
// the overlay within one half-float step, at a tint-1 texel (W = 1) and an
// untinted one. The overlay subtracts only D_static, which no glow reaches.
#[test]
fn scripted_and_animated_glow_pass_through_the_overlay_unchanged() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let roster = vec![pulse()];
    let area = AreaGlowLight {
        glow: 2.0,
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        compose_slot: Some(0),
        faces: vec![Face::Ceiling.index() as u32],
    };
    let room = Room {
        roster: roster.clone(),
        glow_ids: [1, 0, AREA_GLOW_FIRST_ENTRY as u32],
        emissive_volumes: vec![EmissiveVolumeRecord {
            min: 0.0,
            max: 1.0,
            color: [1.0, 1.0, 1.0],
            origin: [0.0, 0.0, 5.0],
            faces: vec![Face::Floor.index() as u32],
            tags: Vec::new(),
        }],
        area_lights: vec![area.clone()],
        ..Room::new([skylight, skylight, skylight])
    };
    // The pulse dark at t = 0 and mid-way up at t = 0.5.
    let factor = |time: f32| {
        let light = &roster[0];
        area_glow_factor(
            area_glow_entry(&area),
            Some(AnimatedLightScaleDescriptor {
                period: light.period,
                phase: light.phase,
                base_color: light.base_color,
                brightness: &light.brightness,
                color: &light.color,
                is_active: true,
            }),
            time,
        )
    };
    let (dark_time, mid_time) = (0.0, 0.5);
    assert!(
        factor(dark_time).iter().all(|&c| c < 1.0e-3),
        "{:?}",
        factor(dark_time)
    );
    assert!(
        factor(mid_time).iter().all(|&c| c > 0.3 && c < 1.7),
        "{:?}",
        factor(mid_time)
    );
    for tint in [255u8, 254] {
        let material = RoomMaterial {
            slot: if tint == 255 {
                |_, _| [255, 127, 255, 255]
            } else {
                |_, _| [255, 127, 255, 254]
            },
            glow: Some((|_, _| [200, 160, 120, 255], 2.0)),
            ..RoomMaterial::WHITE_SHARP
        };
        room.install(&mut renderer, &material);
        let view = room_view();
        let capture = |renderer: &mut Renderer, glowing: bool| {
            set_volume(renderer, if glowing { [0.6, 0.4, 0.2] } else { [0.0; 3] });
            frames(renderer, view, if glowing { mid_time } else { dark_time })
        };
        let (dark, dark_forward) = capture(&mut renderer, false);
        let (lit, lit_forward) = capture(&mut renderer, true);
        for face in [Face::Floor, Face::Ceiling] {
            let mut glowing = 0;
            for (pixel, _) in face_pixels(view, face, MARGIN) {
                let with = lit.colour(pixel) - dark.colour(pixel);
                let without = lit_forward.colour(pixel) - dark_forward.colour(pixel);
                for c in 0..3 {
                    let slack = half_step(lit.colour(pixel)[c]) + half_step(dark.colour(pixel)[c]);
                    assert!(
                        (with[c] - without[c]).abs() <= slack,
                        "tint {tint}, {face:?} at {pixel}: {} with the overlay against {} without",
                        with[c],
                        without[c]
                    );
                }
                if without.max_element() > 0.1 {
                    glowing += 1;
                }
            }
            assert!(
                glowing > 150,
                "tint {tint}: {face:?} glows on {glowing} pixels"
            );
        }
    }
}
