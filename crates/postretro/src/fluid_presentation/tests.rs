// Camera-eye fluid presentation against head bob, the photosensitivity limiter
// and the mod's screen tint, plus the per-frame fluid surface draws.
// See: context/lib/rendering_pipeline.md §7.3a, §7.8 (Fluid view tint, Photosensitivity limiter)

use std::collections::HashMap;
use std::path::PathBuf;

use glam::Vec3;
use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
};
use postretro_combat_model::{FluidMovementTable, FluidMovementTuning};
use postretro_entities::{SlotTable, SlotValue};
use postretro_foundation::{BobParams, FluidDescriptor, FluidRegistry, ViewFeelParams};
use postretro_level_format::environment_volumes::{
    EnvironmentBrush, EnvironmentPlane, EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
    EnvironmentVolumeRecord, EnvironmentVolumesSection,
};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_clamp::ChannelClamp;
use postretro_render_cpu::flash_limiter::{
    HITCH_CEILING_SECONDS, LimiterFrameInput, flash_limiter_enabled,
};
use postretro_render_cpu::fluid_surface::fog::{FLUID_FOG_BAND, fog_layer_opacity, fog_proximity};
use postretro_render_cpu::fluid_surface::{FluidSurfaceLook, FluidSurfaceRenderCollector};
use postretro_render_cpu::screen_effects::{EffectUniform, pack_effect_uniform};
use postretro_visibility::VisibleCells;

use super::eye::{EYE_FLUID_MARGIN, FLUID_PRESENTATION_RAMP_SECONDS, FluidEye, FluidLook};
use super::{collect_fluid_surface_draws, local_fluid_presentation, surface_look};
use crate::App;
use crate::startup::lifecycle::tests::{level_world, script_ctx, test_app};

/// The pool's top face, metres.
const SURFACE_Y: f32 = 2.0;
const LEVEL_GRAVITY: f32 = -9.81;
const WATER: FluidLook = FluidLook {
    tint: [0.10, 0.30, 0.40, 0.35],
    muffle: 0.8,
};

fn axis_leaf(min: Vec3, max: Vec3) -> ConvexLeaf {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(LeafPlane {
            normal,
            distance: max[axis],
        });
        planes.push(LeafPlane {
            normal: -normal,
            distance: -min[axis],
        });
    }
    ConvexLeaf {
        planes,
        aabb_min: min,
        aabb_max: max,
    }
}

/// One water pool whose top face sits at [`SURFACE_Y`].
fn pool() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(0),
        brushes: vec![axis_leaf(
            Vec3::new(-50.0, -10.0, -50.0),
            Vec3::new(50.0, SURFACE_Y, 50.0),
        )],
    }])
}

fn water_volume(brushes: Vec<ConvexLeaf>) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(0),
        brushes,
    }
}

/// [`pool`] built from two brushes of one entity sharing the face x = 0, the
/// plane [`eye_at`] sits on.
fn seam_pool() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![water_volume(vec![
        axis_leaf(
            Vec3::new(-50.0, -10.0, -50.0),
            Vec3::new(0.0, SURFACE_Y, 50.0),
        ),
        axis_leaf(
            Vec3::new(0.0, -10.0, -50.0),
            Vec3::new(50.0, SURFACE_Y, 50.0),
        ),
    ])])
}

/// [`pool`] split into two stacked entities of one fluid at y = 0.
fn stacked_pool() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![
        water_volume(vec![axis_leaf(
            Vec3::new(-50.0, -10.0, -50.0),
            Vec3::new(50.0, 0.0, 50.0),
        )]),
        water_volume(vec![axis_leaf(
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, SURFACE_Y, 50.0),
        )]),
    ])
}

/// Every pool shape the eye rows run against: one brush, a seam under the
/// eye, and stacked entities.
fn pools() -> [(&'static str, EnvironmentVolumes); 3] {
    [
        ("one brush", pool()),
        ("seam under the eye", seam_pool()),
        ("stacked entities", stacked_pool()),
    ]
}

fn eye_at(y: f32) -> Vec3 {
    Vec3::new(0.0, y, 0.0)
}

/// The head bob content/dev ships, read from its player descriptor so a
/// retune there re-runs these rows against the new amplitude and frequency.
fn dev_head_bob() -> BobParams {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/scripts/player.ts");
    let source = std::fs::read_to_string(&path).expect("content/dev player descriptor reads");
    let bob = &source[source
        .find("bob: {")
        .expect("content/dev declares a head bob")..];
    let bob = &bob[..bob.find('}').expect("bob block closes")];
    let field = |name: &str| -> f32 {
        let at = bob
            .find(&format!("{name}:"))
            .unwrap_or_else(|| panic!("bob.{name}"));
        bob[at + name.len() + 1..]
            .split(',')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("bob.{name} is a number"))
    };
    BobParams {
        vertical_frequency: field("verticalFrequency"),
        lateral_frequency: field("lateralFrequency"),
        vertical_amplitude: field("verticalAmplitude"),
        lateral_amplitude: field("lateralAmplitude"),
        speed_threshold: field("speedThreshold"),
        grounded_only: true,
    }
}

/// The vertical head-bob offsets the real view-feel evaluator produces for a
/// pawn running at `speed` for `seconds` at `fps`, one per frame.
fn bob_offsets(bob: &BobParams, speed: f32, fps: f32, seconds: f32) -> Vec<f32> {
    let params = ViewFeelParams {
        bob: Some(*bob),
        tilt: None,
        sway: None,
        impulse: None,
        slide: None,
        vault: None,
    };
    let mut state = crate::view_feel::ViewFeelState::default();
    let dt = 1.0 / fps;
    (0..(seconds * fps) as usize)
        .map(|_| {
            crate::view_feel::evaluate(&params, speed, 0.0, true, &mut state, dt, 1.0).bob_vertical
        })
        .collect()
}

/// Drive a [`FluidEye`] in [`pool`] through eye heights, one per frame at
/// `dt`, returning the number of trigger toggles.
fn count_toggles(
    eye: &mut FluidEye,
    volumes: &EnvironmentVolumes,
    heights: &[f32],
    dt: f32,
) -> usize {
    let view = EnvironmentView::new(LEVEL_GRAVITY, volumes, &[]);
    let mut toggles = 0;
    for &y in heights {
        let before = eye.active_volume();
        eye.update(view, eye_at(y), dt, |_| Some(WATER));
        toggles += usize::from(eye.active_volume() != before);
    }
    toggles
}

/// Count WCAG transitions of `series` (one value per frame at `dt`) in the
/// worst one-second window, as `rendering_pipeline.md` §7.8 counts them: a
/// same-sign change from the last extremum reaching 0.1 with the darker state
/// below 0.8, where a reversal ends an excursion only once it reaches 0.1.
fn worst_transitions(series: &[f32], dt: f32) -> usize {
    let mut times = Vec::new();
    let (mut ext, mut peak, mut dir, mut counted) = (series[0], series[0], 0.0f32, false);
    for (n, &v) in series.iter().enumerate().skip(1) {
        let beyond = (v - peak) * dir;
        if dir == 0.0 {
            if v != peak {
                dir = (v - peak).signum();
                peak = v;
            }
        } else if beyond > 0.0 {
            peak = v;
        } else if -beyond >= 0.1 {
            ext = peak;
            peak = v;
            dir = -dir;
            counted = false;
        }
        if !counted && (v - ext).abs() >= 0.1 && v.min(ext) < 0.8 {
            counted = true;
            times.push(n as f32 * dt);
        }
    }
    (0..times.len())
        .map(|i| {
            times[i..]
                .iter()
                .take_while(|&&t| t < times[i] + 1.0)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// One presented frame through the resolve's own path: the eye advances, the
/// slots and the fluid tint pack, and the channel clamp limits.
fn present(
    (eye, clamp): (&mut FluidEye, &mut ChannelClamp),
    view: EnvironmentView<'_>,
    y: f32,
    dt: f32,
    look: FluidLook,
    slots: &HashMap<String, SlotValue>,
    limiter: bool,
) -> EffectUniform {
    eye.update(view, eye_at(y), dt, |_| Some(look));
    let mut uniform = pack_effect_uniform(slots, eye.tint());
    let frame = clamp.begin_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        limiter,
    );
    clamp.apply(&mut uniform, &frame);
    uniform
}

// The content/dev head bob stays inside the band, so treading at the surface
// never flickers the tint.
#[test]
fn eye_bobbing_across_the_surface_at_dev_head_bob_toggles_the_tint_at_most_once() {
    let bob = dev_head_bob();
    assert!(
        bob.vertical_amplitude < EYE_FLUID_MARGIN,
        "the hysteresis margin {EYE_FLUID_MARGIN} m must exceed content/dev's bob amplitude {} m",
        bob.vertical_amplitude
    );
    // Just above the threshold (full amplitude, slowest bob) to a sprint.
    let full = bob.speed_threshold + crate::view_feel::BOB_EASE_IN_BAND;
    for (shape, volumes) in pools() {
        for speed in [full, full + 5.0, full + 20.0] {
            for fps in [30.0, 60.0, 144.0] {
                let offsets = bob_offsets(&bob, speed, fps, 10.0);
                let peak = offsets.iter().fold(0.0f32, |p, o| p.max(o.abs()));
                assert!(
                    (peak - bob.vertical_amplitude).abs() < 0.01,
                    "the eye really bobs: peak {peak} at {speed} m/s"
                );
                let centred: Vec<f32> = offsets.iter().map(|o| SURFACE_Y + o).collect();

                let mut from_air = FluidEye::default();
                let toggles = count_toggles(&mut from_air, &volumes, &centred, 1.0 / fps);
                assert!(
                    toggles <= 1,
                    "{shape}, from air: {toggles} toggles at {speed} m/s, {fps} fps"
                );

                let mut from_under = FluidEye::default();
                count_toggles(&mut from_under, &volumes, &[SURFACE_Y - 1.0], 1.0 / fps);
                assert!(from_under.active_volume().is_some(), "{shape}: under is on");
                let toggles = count_toggles(&mut from_under, &volumes, &centred, 1.0 / fps);
                assert!(
                    toggles <= 1,
                    "{shape}, from under: {toggles} toggles at {speed} m/s, {fps} fps"
                );
            }
        }
    }
}

// An eye strobing across the band at 5 Hz presents at most three tint
// flashes a second, counted on presented-frame time, at any frame rate.
#[test]
fn eye_oscillating_past_the_band_at_5hz_presents_at_most_three_tint_flashes_a_second() {
    let volumes = pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    // A full-strength tint is the worst case the limiter sees.
    let look = FluidLook {
        tint: [0.1, 0.3, 0.4, 1.0],
        muffle: 1.0,
    };
    for fps in [30.0, 60.0, 144.0] {
        let dt = 1.0 / fps;
        let heights: Vec<f32> = (0..(3.0 * fps) as usize)
            .map(|n| SURFACE_Y - 0.2 * (std::f32::consts::TAU * 5.0 * n as f32 * dt).sin())
            .collect();
        let run = |limiter: bool| {
            let (mut eye, mut clamp) = (FluidEye::default(), ChannelClamp::default());
            heights
                .iter()
                .map(|&y| {
                    present(
                        (&mut eye, &mut clamp),
                        view,
                        y,
                        dt,
                        look,
                        &HashMap::new(),
                        limiter,
                    )
                    .fluid_tint[3]
                })
                .collect::<Vec<f32>>()
        };
        let raw = worst_transitions(&run(false), dt);
        assert!(
            raw > 6,
            "{fps} fps: the unlimited strobe is {raw} transitions a second"
        );
        let limited = worst_transitions(&run(true), dt);
        assert!(
            limited <= 6,
            "{fps} fps: {limited} transitions in one second (> 3 flashes)"
        );
    }
}

// With the limiter off nothing is held, yet the bob still toggles at
// most once and the toggle still ramps.
#[test]
fn with_the_limiter_off_the_bob_toggles_at_most_once_and_each_toggle_ramps() {
    let bob = dev_head_bob();
    let volumes = pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let fps = 60.0;
    let dt = 1.0 / fps;
    let speed = bob.speed_threshold + 2.0;
    let offsets = bob_offsets(&bob, speed, fps, 5.0);
    // Treading at the surface for two seconds, sinking 0.5 m over one, then
    // bobbing there.
    let centre = |n: usize| {
        let t = n as f32 * dt;
        SURFACE_Y - 0.5 * ((t - 2.0) / 1.0).clamp(0.0, 1.0)
    };
    let (mut eye, mut clamp) = (FluidEye::default(), ChannelClamp::default());
    let mut toggles = 0;
    let mut strengths = Vec::new();
    for (n, offset) in offsets.iter().enumerate() {
        let before = eye.active_volume();
        let packed = present(
            (&mut eye, &mut clamp),
            view,
            centre(n) + offset,
            dt,
            WATER,
            &HashMap::new(),
            false,
        );
        toggles += usize::from(eye.active_volume() != before);
        assert_eq!(packed.fluid_tint, eye.tint(), "off passes the tint through");
        strengths.push(packed.fluid_tint[3]);
    }
    assert_eq!(toggles, 1, "sinking past the band toggles once");
    let full = WATER.tint[3];
    let per_frame = full * dt / FLUID_PRESENTATION_RAMP_SECONDS;
    for pair in strengths.windows(2) {
        assert!(pair[1] >= pair[0], "the toggle ramps in monotonically");
        assert!(
            pair[1] - pair[0] <= per_frame + 1e-6,
            "never a step: {pair:?}"
        );
    }
    let ramp_frames = strengths.iter().filter(|&&s| s > 0.0 && s < full).count();
    assert!(
        ramp_frames as f32 >= FLUID_PRESENTATION_RAMP_SECONDS * fps - 2.0,
        "the toggle ramps over {ramp_frames} frames"
    );
    assert_eq!(strengths.last(), Some(&full));
}

#[test]
fn a_reversal_mid_ramp_turns_the_tint_and_muffle_back_without_a_step() {
    let volumes = pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let dt = 1.0 / 60.0;
    let mut eye = FluidEye::default();
    let mut ramps = Vec::new();
    for n in 0..60 {
        // Under for a tenth of a second, then out again.
        let y = if n < 6 {
            SURFACE_Y - 0.5
        } else {
            SURFACE_Y + 0.5
        };
        eye.update(view, eye_at(y), dt, |_| Some(WATER));
        ramps.push(eye.ramp());
        assert!((eye.muffle() - WATER.muffle * eye.ramp()).abs() < 1e-6);
        if eye.ramp() > 0.0 {
            // The held look keeps the fluid's color through the ramp out.
            assert_eq!(&eye.tint()[..3], &WATER.tint[..3]);
        }
    }
    let step = dt / FLUID_PRESENTATION_RAMP_SECONDS;
    let turned = ramps[5];
    assert!(
        (turned - 6.0 * step).abs() < 1e-5,
        "turned back at {turned}"
    );
    for pair in ramps.windows(2) {
        assert!((pair[1] - pair[0]).abs() <= step + 1e-6);
    }
    assert_eq!(ramps.last(), Some(&0.0));
    assert_eq!(eye.tint(), [0.0; 4], "fully dry packs no tint at all");
}

fn write_slot(slots: &mut SlotTable, name: &str, value: [f32; 4]) {
    slots
        .get_mut(name)
        .unwrap_or_else(|| panic!("{name}"))
        .write_value(Some(SlotValue::Array(value.to_vec())));
}

fn screen_slots(snapshot: &HashMap<String, SlotValue>) -> Vec<(String, SlotValue)> {
    let mut screen: Vec<_> = snapshot
        .iter()
        .filter(|(name, _)| name.starts_with("screen."))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    screen.sort_by(|a, b| a.0.cmp(&b.0));
    screen
}

// The fluid tint composes over the mod's tint, a script write while
// under takes effect without clearing it, leaving restores the mod's tint, and
// no `screen.*` slot ever holds anything but what the mod last wrote.
#[test]
fn fluid_tint_composes_over_the_mods_screen_tint_and_never_writes_a_screen_slot() {
    let volumes = pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let dt = 1.0 / 60.0;
    let mut slots = SlotTable::new();
    let mod_tint = [0.8, 0.2, 0.1, 0.3];
    write_slot(&mut slots, "screen.flash", mod_tint);
    let (mut eye, mut clamp) = (FluidEye::default(), ChannelClamp::default());
    let mut frames = |slots: &SlotTable, y: f32, count: usize| {
        let mut last = EffectUniform::default();
        for _ in 0..count {
            let written = App::build_ui_slot_snapshot(slots);
            last = present(
                (&mut eye, &mut clamp),
                view,
                y,
                dt,
                WATER,
                &written,
                flash_limiter_enabled(&written),
            );
            assert_eq!(
                screen_slots(&App::build_ui_slot_snapshot(slots)),
                screen_slots(&written),
                "presenting the fluid writes no screen slot"
            );
        }
        last
    };
    let under = SURFACE_Y - 0.5;
    let above = SURFACE_Y + 0.5;

    let dry = frames(&slots, above, 30);
    assert_eq!(dry.flash, mod_tint);
    assert_eq!(dry.fluid_tint[3], 0.0);

    let wet = frames(&slots, under, 30);
    assert_eq!(
        wet.flash, mod_tint,
        "the mod's tint is untouched underneath"
    );
    assert_eq!(
        wet.fluid_tint, WATER.tint,
        "the fluid tint composes over it"
    );

    let left = frames(&slots, above, 30);
    assert_eq!(
        left,
        pack_effect_uniform(&App::build_ui_slot_snapshot(&slots), [0.0; 4]),
        "leaving restores exactly the mod's own uniform"
    );

    frames(&slots, under, 30);
    let rewritten = [0.0, 0.0, 1.0, 0.6];
    write_slot(&mut slots, "screen.flash", rewritten);
    let wet = frames(&slots, under, 30);
    assert_eq!(
        wet.flash, rewritten,
        "a script write under the surface lands"
    );
    assert_eq!(
        wet.fluid_tint, WATER.tint,
        "and does not clear the fluid tint"
    );
    let snapshot = App::build_ui_slot_snapshot(&slots);
    assert_eq!(
        snapshot.get("screen.flash"),
        Some(&SlotValue::Array(rewritten.to_vec())),
        "the slot keeps the value the mod last wrote"
    );
}

fn surface_face(y: f32) -> EnvironmentSurfaceFace {
    let vertex = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, y, z],
        uv: [0.0, 0.0],
    };
    EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            vertex(0.0, 0.0),
            vertex(0.0, 1.0),
            vertex(1.0, 1.0),
            vertex(1.0, 0.0),
        ],
    }
}

fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = [0.0; 3];
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal,
            distance: max[axis],
        });
        normal[axis] = -1.0;
        planes.push(EnvironmentPlane {
            normal,
            distance: -min[axis],
        });
    }
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

fn fluid_record(fluid: &str, x: f32) -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        fluid: fluid.to_string(),
        brushes: vec![box_brush(
            Vec3::new(x, 0.0, 0.0),
            Vec3::new(x + 1.0, 1.0, 1.0),
        )],
        surface_faces: if fluid.is_empty() {
            Vec::new()
        } else {
            vec![surface_face(1.0)]
        },
    }
}

/// Install `world`'s environment the way the level install does: resolve its
/// fluid names against the active table and keep the world.
fn install_environment(app: &mut App, world: LevelWorld) {
    app.environment_fluids = app.active_environment_fluids();
    app.environment_volumes = crate::startup::environment_install::build_level_environment_volumes(
        &world,
        app.environment_fluids.names(),
    );
    app.level = Some(world);
}

fn descriptor(name: &str, surface_opacity: f32) -> FluidDescriptor {
    FluidDescriptor {
        name: name.to_string(),
        viscosity: 2.0,
        view_tint: WATER.tint,
        muffle: WATER.muffle,
        surface_opacity,
        enter_sound: None,
        exit_sound: None,
        fog_distance: None,
        fog_color: None,
        wave_period: None,
        wave_amplitude: 8.0,
        wave_length: [64.0, 80.0],
    }
}

// App wiring: an installed level's fluid surfaces draw through the
// per-frame path only where the fluid has a local presentation.
#[test]
fn installed_fluid_surfaces_draw_only_where_the_fluid_has_a_local_presentation() {
    let mut app = test_app();
    script_ctx(&app).data_registry.borrow_mut().fluids =
        FluidRegistry::from_declarations("test", [Ok(descriptor("water", 0.6))]);
    let mut world = level_world("pool", 1);
    world.environment_volumes = EnvironmentVolumesSection {
        level_gravity: LEVEL_GRAVITY,
        volumes: vec![
            fluid_record("water", 0.0),
            fluid_record("sludge", 4.0),
            fluid_record("", 8.0),
        ],
    };
    install_environment(&mut app, world);

    let local = script_ctx(&app).data_registry.borrow().fluids.clone();
    let draws = collect_fluid_surface_draws(
        &mut app.fluid_surface_render,
        app.level.as_ref().expect("installed"),
        &VisibleCells::DrawAll,
        Vec3::ZERO,
        &app.environment_volumes,
        &app.environment_fluids,
        &local,
    )
    .to_vec();
    assert_eq!(draws.len(), 1, "{draws:?}");
    assert_eq!(draws[0].volume_index, 0);
    assert!((draws[0].params.opacity() - 0.6).abs() < 1e-6);

    // A connected client resolves the host's table: `sludge` exists there but
    // not in this machine's manifest, so it shows none.
    let host_table = FluidMovementTable::new(vec![
        FluidMovementTuning {
            name: "water".into(),
            viscosity: 2.0,
        },
        FluidMovementTuning {
            name: "sludge".into(),
            viscosity: 6.0,
        },
    ])
    .expect("valid host table");
    let fluid_volume = |fluid: u16, x: f32| EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(fluid),
        brushes: vec![axis_leaf(
            Vec3::new(x, 0.0, 0.0),
            Vec3::new(x + 1.0, 1.0, 1.0),
        )],
    };
    let volumes = EnvironmentVolumes::build(vec![fluid_volume(0, 0.0), fluid_volume(1, 4.0)]);
    assert!(local_fluid_presentation(&volumes, &host_table, &local, 0).is_some());
    assert!(local_fluid_presentation(&volumes, &host_table, &local, 1).is_none());
    let mut collector = FluidSurfaceRenderCollector::new();
    let records = [fluid_record("water", 0.0), fluid_record("sludge", 4.0)];
    let draws = collector.collect(
        &records,
        &VisibleCells::DrawAll,
        |_| None,
        Vec3::ZERO,
        |v| local_fluid_presentation(&volumes, &host_table, &local, v).map(surface_look),
    );
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].volume_index, 0);
}

// The App's per-frame hook reads the eye's fluid through the installed
// volumes and the local manifest, and level unload drops it.
#[test]
fn app_fluid_eye_presents_the_local_fluid_and_unload_clears_it() {
    let mut app = test_app();
    script_ctx(&app).data_registry.borrow_mut().fluids =
        FluidRegistry::from_declarations("test", [Ok(descriptor("water", 0.6))]);
    let mut world = level_world("pool", 1);
    world.environment_volumes = EnvironmentVolumesSection {
        level_gravity: LEVEL_GRAVITY,
        volumes: vec![fluid_record("water", 0.0)],
    };
    install_environment(&mut app, world);

    let under = Vec3::new(0.5, 0.5, 0.5);
    for _ in 0..30 {
        app.update_fluid_eye(under, 1.0 / 60.0);
    }
    assert_eq!(app.fluid_eye.tint(), WATER.tint);
    assert_eq!(app.fluid_eye.take_muffle_change(), Some(WATER.muffle));
    assert_eq!(
        app.fluid_eye.take_muffle_change(),
        None,
        "written on change only"
    );

    app.unload_level();
    assert_eq!(app.fluid_eye.tint()[3], 0.0);
    assert_eq!(app.fluid_eye.muffle(), 0.0);
}

// While on, moving straight into an adjacent fluid volume follows the same
// band at the seam between them: the eye switches once, without ramping out.
#[test]
fn eye_moving_between_adjacent_fluid_volumes_switches_once_at_the_band() {
    let pool_at = |fluid: u16, min_x: f32, max_x: f32| EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(fluid),
        brushes: vec![axis_leaf(
            Vec3::new(min_x, -10.0, -10.0),
            Vec3::new(max_x, SURFACE_Y, 10.0),
        )],
    };
    let volumes = EnvironmentVolumes::build(vec![pool_at(0, -10.0, 0.0), pool_at(1, 0.0, 10.0)]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let sludge = FluidLook {
        tint: [0.3, 0.4, 0.1, 0.6],
        muffle: 1.0,
    };
    let look_of = |volume: usize| Some(if volume == 0 { WATER } else { sludge });
    let dt = 1.0 / 60.0;
    let mut eye = FluidEye::default();
    let deep = SURFACE_Y - 1.0;
    for _ in 0..30 {
        eye.update(view, Vec3::new(-1.0, deep, 0.0), dt, look_of);
    }
    assert_eq!(eye.active_volume(), Some(0));

    // Swaying across the seam inside the band never switches.
    for n in 0..120 {
        let x = 0.05 * (n as f32 * 0.3).sin();
        eye.update(view, Vec3::new(x, deep, 0.0), dt, look_of);
        assert_eq!(eye.active_volume(), Some(0), "x = {x}");
    }
    // Past the band, it switches once, at full ramp, to the other fluid's look.
    let mut switches = 0;
    for n in 0..60 {
        let before = eye.active_volume();
        eye.update(view, Vec3::new(n as f32 * 0.01, deep, 0.0), dt, look_of);
        switches += usize::from(eye.active_volume() != before);
        assert_eq!(eye.ramp(), 1.0, "no ramp out at the seam");
    }
    assert_eq!(switches, 1);
    assert_eq!(eye.active_volume(), Some(1));
    assert_eq!(eye.tint(), sludge.tint);
}

// Regression: the eye depth was a per-brush plane depth, which reads 0 on a seam
// inside a pool, so an eye sinking at the seam never turned the tint on.
#[test]
fn eye_sinking_at_a_two_brush_pools_internal_seam_turns_the_tint_on_8cm_under_the_surface() {
    let volumes = seam_pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut eye = FluidEye::default();
    // Down from 20 cm above the surface to 30 cm under, 1 mm a frame.
    let mut turned_on = None;
    for step in 0..=500 {
        let y = SURFACE_Y + 0.2 - step as f32 * 0.001;
        eye.update(view, eye_at(y), 1.0 / 60.0, |_| Some(WATER));
        if eye.active_volume().is_some() && turned_on.is_none() {
            turned_on = Some(y);
        }
        if turned_on.is_none() {
            assert_eq!(eye.ramp(), 0.0, "off above the band at y = {y}");
        }
    }
    let on_at = turned_on.expect("the tint turns on under the seam");
    assert!(
        ((SURFACE_Y - on_at) - EYE_FLUID_MARGIN).abs() < 0.0015,
        "on {} m under the surface",
        SURFACE_Y - on_at
    );
    // And off again 8 cm above it on the way up.
    let mut turned_off = None;
    for step in 0..=500 {
        let y = SURFACE_Y - 0.3 + step as f32 * 0.001;
        eye.update(view, eye_at(y), 1.0 / 60.0, |_| Some(WATER));
        if eye.active_volume().is_none() && turned_off.is_none() {
            turned_off = Some(y);
        }
    }
    let off_at = turned_off.expect("the tint turns off above the seam");
    assert!(((off_at - SURFACE_Y) - EYE_FLUID_MARGIN).abs() < 0.0015);
}

// A hitch frame advances the ramp by at most the limiter's hitch ceiling, so
// entry is never held by the limiter's rate cap, which allows that much too.
#[test]
fn a_hitch_frame_steps_the_tint_ramp_no_further_than_the_limiter_admits() {
    let volumes = pool();
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let full = FluidLook {
        tint: [0.1, 0.3, 0.4, 1.0],
        muffle: 1.0,
    };
    for look in [WATER, full] {
        let (mut eye, mut clamp) = (FluidEye::default(), ChannelClamp::default());
        let slots = HashMap::new();
        for _ in 0..10 {
            present(
                (&mut eye, &mut clamp),
                view,
                SURFACE_Y + 0.5,
                1.0 / 60.0,
                look,
                &slots,
                true,
            );
        }
        // Dive under on half-second hitches, then out the same way.
        let mut strengths = Vec::new();
        for y in [SURFACE_Y - 0.5; 10]
            .into_iter()
            .chain([SURFACE_Y + 0.5; 10])
        {
            let packed = present((&mut eye, &mut clamp), view, y, 0.5, look, &slots, true);
            assert_eq!(
                packed.fluid_tint,
                eye.tint(),
                "the limiter never holds the ramp on a hitch"
            );
            strengths.push(packed.fluid_tint[3]);
        }
        let per_hitch = look.tint[3] * HITCH_CEILING_SECONDS / FLUID_PRESENTATION_RAMP_SECONDS;
        assert!((strengths[0] - per_hitch).abs() < 1e-6, "{strengths:?}");
        for pair in strengths.windows(2) {
            assert!((pair[1] - pair[0]).abs() <= per_hitch + 1e-6, "{pair:?}");
        }
    }
}

// Switching into a fluid this machine does not present ramps the held look
// out rather than dropping it in one frame.
#[test]
fn switching_into_a_fluid_with_no_local_presentation_ramps_the_held_look_out() {
    let pool_at = |fluid: u16, min_x: f32, max_x: f32| EnvironmentVolume {
        fluid: Some(fluid),
        ..water_volume(vec![axis_leaf(
            Vec3::new(min_x, -10.0, -10.0),
            Vec3::new(max_x, SURFACE_Y, 10.0),
        )])
    };
    let volumes = EnvironmentVolumes::build(vec![pool_at(0, -10.0, 0.0), pool_at(1, 0.0, 10.0)]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    // Fluid 1 is undeclared here: no local presentation.
    let look_of = |volume: usize| (volume == 0).then_some(WATER);
    let dt = 1.0 / 60.0;
    let deep = SURFACE_Y - 1.0;
    let mut eye = FluidEye::default();
    for _ in 0..30 {
        eye.update(view, Vec3::new(-1.0, deep, 0.0), dt, look_of);
    }
    assert_eq!(eye.tint(), WATER.tint);

    let step = dt / FLUID_PRESENTATION_RAMP_SECONDS;
    let mut previous = eye.ramp();
    for frame in 0..30 {
        eye.update(view, Vec3::new(1.0, deep, 0.0), dt, look_of);
        assert_eq!(
            eye.active_volume(),
            Some(1),
            "the eye is in the other fluid"
        );
        assert!(
            (previous - eye.ramp() - step).abs() < 1e-5 || eye.ramp() == 0.0,
            "frame {frame}: ramp {} after {previous}",
            eye.ramp()
        );
        if eye.ramp() > 0.0 {
            assert_eq!(&eye.tint()[..3], &WATER.tint[..3], "the held look");
            assert!((eye.tint()[3] - WATER.tint[3] * eye.ramp()).abs() < 1e-6);
        }
        previous = eye.ramp();
    }
    assert_eq!(eye.tint(), [0.0; 4]);
    assert_eq!(eye.muffle(), 0.0);
}

// Suspend drops the surface; no tint or muffle may outlive it.
#[test]
fn suspend_clears_the_fluid_eye_presentation() {
    let main = include_str!("../main.rs");
    let start = main
        .find("fn suspended(")
        .expect("main.rs has a suspend handler");
    let body = &main[start..];
    let body = &body[..body.find("\n    fn ").unwrap_or(body.len())];
    assert!(
        body.contains("self.clear_fluid_eye_presentation();"),
        "suspended() clears the fluid eye presentation"
    );

    let mut app = test_app();
    app.fluid_eye = {
        let volumes = pool();
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let mut eye = FluidEye::default();
        for _ in 0..30 {
            eye.update(view, eye_at(SURFACE_Y - 1.0), 1.0 / 60.0, |_| Some(WATER));
        }
        eye
    };
    assert_eq!(app.fluid_eye.tint(), WATER.tint);
    app.clear_fluid_eye_presentation();
    assert_eq!(app.fluid_eye.tint(), [0.0; 4]);
    assert_eq!(app.fluid_eye.muffle(), 0.0);
}

// An eye swaying out through a pool's side face keeps the band there, even
// with a lower basin of the same fluid under it that the vertical line meets.
#[test]
fn eye_swaying_across_a_pools_side_face_above_a_lower_basin_stays_on_inside_the_band() {
    let volumes = EnvironmentVolumes::build(vec![
        water_volume(vec![axis_leaf(
            Vec3::new(-10.0, -2.0, -10.0),
            Vec3::new(0.0, SURFACE_Y, 10.0),
        )]),
        water_volume(vec![axis_leaf(
            Vec3::new(0.0, -10.0, -10.0),
            Vec3::new(10.0, -0.5, 10.0),
        )]),
    ]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    // Fixture: the vertical line beside the pool meets the basin 1.5 m down.
    let beside = Vec3::new(0.05, SURFACE_Y - 1.0, 0.0);
    let vertical = postretro_collision::fluid_volume_signed_depth(view, 0, beside)
        .expect("the basin is on the vertical line");
    assert!((vertical + 1.5).abs() < 1e-4, "{vertical}");
    let dt = 1.0 / 60.0;
    let y = SURFACE_Y - 1.0;
    let mut eye = FluidEye::default();
    for _ in 0..30 {
        eye.update(view, Vec3::new(-1.0, y, 0.0), dt, |_| Some(WATER));
    }
    assert!(eye.active_volume().is_some());
    for n in 0..240 {
        let x = 0.05 * (n as f32 * 0.3).sin();
        eye.update(view, Vec3::new(x, y, 0.0), dt, |_| Some(WATER));
        assert!(eye.active_volume().is_some(), "x = {x}");
    }
    // Past the band sideways it turns off, though the basin lies below.
    eye.update(view, Vec3::new(0.09, y, 0.0), dt, |_| Some(WATER));
    assert_eq!(eye.active_volume(), None);
}

// --- Depth fog: the frame fog weight, per-face proximity, and the look -------

/// Whether `eye` is inside the union every [`pools`] shape covers (a face
/// counts as inside, as `fluid_at_eye` reads it).
fn in_pool(eye: Vec3) -> bool {
    eye.x.abs() <= 50.0 && eye.z.abs() <= 50.0 && (-10.0..=SURFACE_Y).contains(&eye.y)
}

/// The frame weight contract: 0 while contained, else one minus the ramp.
fn assert_frame_weight(eye: &FluidEye, contained: bool, context: &str) {
    if contained {
        assert_eq!(eye.fog_weight(), 0.0, "{context}: contained");
    } else {
        assert_eq!(eye.fog_weight(), 1.0 - eye.ramp(), "{context}: dry");
    }
}

// Sinking through the plane, the eye is contained before the trigger turns on
// and while the ramp rises; the weight is 0 throughout. Climbing out, it rises
// as the ramp falls. A side entry already a metre down is contained from its
// first frame inside, through the whole ramp-in.
#[test]
fn frame_fog_weight_is_zero_while_the_eye_is_contained_and_one_minus_the_ramp_otherwise() {
    let dt = 1.0 / 60.0;
    for (shape, volumes) in pools() {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let mut eye = FluidEye::default();
        let down = (0..=500).map(|n| SURFACE_Y + 0.5 - n as f32 * 0.002);
        let hold = std::iter::repeat_n(SURFACE_Y - 0.5, 30);
        let up = (0..=500).map(|n| SURFACE_Y - 0.5 + n as f32 * 0.002);
        let dry = std::iter::repeat_n(SURFACE_Y + 0.5, 30);
        let (mut under_the_trigger, mut fading_in) = (0, 0);
        let mut previous_dry: Option<f32> = None;
        for y in down.chain(hold).chain(up).chain(dry) {
            let at = eye_at(y);
            eye.update(view, at, dt, |_| Some(WATER));
            let contained = in_pool(at);
            assert_frame_weight(&eye, contained, &format!("{shape}, y = {y}"));
            if contained && eye.active_volume().is_none() {
                under_the_trigger += 1;
            }
            if !contained && eye.ramp() > 0.0 {
                fading_in += 1;
                if let Some(previous) = previous_dry {
                    assert!(eye.fog_weight() >= previous, "{shape}: murk fades in");
                }
            }
            previous_dry = (!contained).then_some(eye.fog_weight());
        }
        assert!(
            under_the_trigger > 0,
            "{shape}: the path crosses between the plane and the trigger depth"
        );
        assert!(fading_in > 0, "{shape}: the exit ramps the tint out");
        assert_eq!(eye.fog_weight(), 1.0, "{shape}: fully dry");

        // Side entry, a metre down: on at the face, ramping in from 0.
        let mut eye = FluidEye::default();
        let mut ramping_in = 0;
        for n in 0..60 {
            let at = Vec3::new(50.3 - n as f32 * 0.02, SURFACE_Y - 1.0, 0.0);
            eye.update(view, at, dt, |_| Some(WATER));
            let contained = in_pool(at);
            assert_frame_weight(&eye, contained, &format!("{shape}, side x = {}", at.x));
            if contained && eye.ramp() < 1.0 {
                ramping_in += 1;
            }
        }
        assert!(ramping_in > 1, "{shape}: the side entry ramps in");
    }
}

// Pin P7: after a dive, an eye bobbing about dev water's surface at dev head
// bob (±0.05 m) keeps the frame weight at 0: below the plane it is contained,
// and above it the trigger stays on inside its band, so the ramp holds at 1.
#[test]
fn eye_bobbing_about_the_surface_after_a_dive_keeps_the_frame_fog_weight_at_zero() {
    let bob = dev_head_bob();
    let full = bob.speed_threshold + crate::view_feel::BOB_EASE_IN_BAND;
    for (shape, volumes) in pools() {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        for speed in [full, full + 5.0] {
            for fps in [30.0, 60.0, 144.0] {
                let dt = 1.0 / fps;
                let mut eye = FluidEye::default();
                for _ in 0..(fps as usize) {
                    eye.update(view, eye_at(SURFACE_Y - 1.0), dt, |_| Some(WATER));
                }
                assert_eq!(eye.ramp(), 1.0, "{shape}: dived in");
                let offsets = bob_offsets(&bob, speed, fps, 10.0);
                assert!(offsets.iter().any(|&o| o > 0.0) && offsets.iter().any(|&o| o < 0.0));
                for offset in offsets {
                    let y = SURFACE_Y + offset;
                    eye.update(view, eye_at(y), dt, |_| Some(WATER));
                    assert_eq!(eye.ramp(), 1.0, "{shape}, y = {y}: the tint holds");
                    assert_eq!(
                        eye.fog_weight(),
                        0.0,
                        "{shape}, y = {y}, {speed} m/s, {fps} fps"
                    );
                }
            }
        }
    }
}

// Pin P8: a swimmer in volume A facing the shared face of volume B (same
// fluid or another, side by side or stacked) has frame weight 0, so B's face,
// outside-facing toward the camera at full proximity, draws no fog.
#[test]
fn swimmer_facing_the_shared_face_of_a_second_fluid_volume_has_frame_fog_weight_zero() {
    let volume = |fluid: u16, min: Vec3, max: Vec3| EnvironmentVolume {
        fluid: Some(fluid),
        ..water_volume(vec![axis_leaf(min, max)])
    };
    let side_by_side = |b_fluid: u16| {
        EnvironmentVolumes::build(vec![
            volume(
                0,
                Vec3::new(-10.0, -10.0, -10.0),
                Vec3::new(0.0, SURFACE_Y, 10.0),
            ),
            volume(
                b_fluid,
                Vec3::new(0.0, -10.0, -10.0),
                Vec3::new(10.0, SURFACE_Y, 10.0),
            ),
        ])
    };
    let stacked = |b_fluid: u16| {
        EnvironmentVolumes::build(vec![
            volume(
                0,
                Vec3::new(-10.0, 0.0, -10.0),
                Vec3::new(10.0, SURFACE_Y, 10.0),
            ),
            volume(
                b_fluid,
                Vec3::new(-10.0, -10.0, -10.0),
                Vec3::new(10.0, 0.0, 10.0),
            ),
        ])
    };
    let sludge = FluidLook {
        tint: [0.25, 0.3, 0.08, 0.85],
        muffle: 0.95,
    };
    // (case, volumes, eye in A, a point on B's shared face, B's outward normal)
    let cases = [
        (
            "same fluid, side by side",
            side_by_side(0),
            Vec3::new(-0.5, SURFACE_Y - 1.0, 0.0),
            Vec3::new(0.0, SURFACE_Y - 1.5, 1.0),
            Vec3::NEG_X,
        ),
        (
            "another fluid, side by side",
            side_by_side(1),
            Vec3::new(-0.5, SURFACE_Y - 1.0, 0.0),
            Vec3::new(0.0, SURFACE_Y - 1.5, 1.0),
            Vec3::NEG_X,
        ),
        (
            "same fluid, stacked",
            stacked(0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 3.0),
            Vec3::Y,
        ),
        (
            "water over sludge",
            stacked(1),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 3.0),
            Vec3::Y,
        ),
    ];
    let dt = 1.0 / 60.0;
    for (case, volumes, at, face, outward) in cases {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let proximity = fog_proximity(at, face, outward);
        assert_eq!(
            proximity, 1.0,
            "{case}: B's face is outside-facing and near"
        );
        let mut eye = FluidEye::default();
        for frame in 0..60 {
            eye.update(view, at, dt, |volume| {
                Some(if volume == 0 { WATER } else { sludge })
            });
            assert_eq!(eye.fog_weight(), 0.0, "{case}, frame {frame}");
            for transmittance in [0.0, 0.05, 0.5] {
                assert_eq!(
                    fog_layer_opacity(eye.fog_weight(), proximity, transmittance),
                    0.0,
                    "{case}: B's face draws no fog"
                );
            }
        }
    }
}

/// A water volume reaching 2 m under its surface face at y = 1.
fn deep_water_record() -> EnvironmentVolumeRecord {
    EnvironmentVolumeRecord {
        brushes: vec![box_brush(
            Vec3::new(0.0, -2.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        )],
        ..fluid_record("water", 0.0)
    }
}

// Pin P11: an eye placed a metre under on one frame (teleport, respawn, or a
// level install after unload reset the ramp) has frame weight 0 on that first
// frame while the tint ramp is still near 0; and an eye crossing into an
// adjacent fluid this machine does not present stays at 0 through the whole
// ramp-out.
#[test]
fn eye_placed_under_a_fluid_or_in_an_unpresented_one_has_frame_fog_weight_zero_whatever_the_ramp() {
    let dt = 1.0 / 60.0;
    let first_step = dt / FLUID_PRESENTATION_RAMP_SECONDS;
    for (shape, volumes) in pools() {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let mut eye = FluidEye::default();
        eye.update(view, eye_at(SURFACE_Y - 1.0), dt, |_| Some(WATER));
        assert!(
            (eye.ramp() - first_step).abs() < 1e-6,
            "{shape}: ramp near 0"
        );
        assert_eq!(eye.fog_weight(), 0.0, "{shape}: first frame");
    }

    // Through the App: a level installs with the eye already a metre under,
    // and again after unload has reset the ramp.
    let mut app = test_app();
    script_ctx(&app).data_registry.borrow_mut().fluids =
        FluidRegistry::from_declarations("test", [Ok(descriptor("water", 0.6))]);
    let under = Vec3::new(0.5, 0.0, 0.5);
    for install in ["first install", "after unload"] {
        let mut world = level_world("pool", 1);
        world.environment_volumes = EnvironmentVolumesSection {
            level_gravity: LEVEL_GRAVITY,
            volumes: vec![deep_water_record()],
        };
        install_environment(&mut app, world);
        assert_eq!(app.fluid_eye.ramp(), 0.0, "{install}: the ramp starts dry");
        app.update_fluid_eye(under, dt);
        assert!((app.fluid_eye.ramp() - first_step).abs() < 1e-6);
        assert_eq!(app.fluid_eye.fog_weight(), 0.0, "{install}: first frame");
        app.unload_level();
        assert_eq!(app.fluid_eye.fog_weight(), 1.0, "unload reads dry");
    }

    // Into a neighbour with no local presentation: the held look ramps out,
    // and the weight stays 0 through all of it and after.
    let pool_at = |fluid: u16, min_x: f32, max_x: f32| EnvironmentVolume {
        fluid: Some(fluid),
        ..water_volume(vec![axis_leaf(
            Vec3::new(min_x, -10.0, -10.0),
            Vec3::new(max_x, SURFACE_Y, 10.0),
        )])
    };
    let volumes = EnvironmentVolumes::build(vec![pool_at(0, -10.0, 0.0), pool_at(1, 0.0, 10.0)]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let look_of = |volume: usize| (volume == 0).then_some(WATER);
    let deep = SURFACE_Y - 1.0;
    let mut eye = FluidEye::default();
    for _ in 0..30 {
        eye.update(view, Vec3::new(-1.0, deep, 0.0), dt, look_of);
    }
    assert_eq!(eye.ramp(), 1.0);
    let mut ramping_out = 0;
    for frame in 0..30 {
        eye.update(view, Vec3::new(1.0, deep, 0.0), dt, look_of);
        ramping_out += usize::from(eye.ramp() > 0.0);
        assert_eq!(eye.fog_weight(), 0.0, "frame {frame}, ramp {}", eye.ramp());
    }
    assert!(
        ramping_out > 10,
        "the held look ramps out over several frames"
    );
    assert_eq!(eye.ramp(), 0.0);
}

// The fog's proximity band is the tint's hysteresis band, so murk crossfades
// with the tint over the same depth.
#[test]
fn the_fog_proximity_band_is_the_eye_tint_band() {
    assert_eq!(FLUID_FOG_BAND, EYE_FLUID_MARGIN);
}

// An eye lowered from one band-width above dev water's surface to the plane,
// before the tint triggers, takes the surface's proximity from 1 to 0
// monotonically at frame weight 1. An eye bobbing across the plane at dev
// head bob from a standing start has proximity 0 at every sample under the
// plane, and proximity (and the fog factor) moves between frames by at most
// the height change ÷ the band width.
#[test]
fn surface_proximity_ramps_monotonically_to_the_plane_and_never_jumps_under_head_bob() {
    let face = Vec3::new(3.0, SURFACE_Y, -2.0);
    let dt = 1.0 / 60.0;
    for (shape, volumes) in pools() {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let mut eye = FluidEye::default();
        let steps = 80;
        let mut proximities = Vec::new();
        for k in 0..=steps {
            let y = SURFACE_Y + FLUID_FOG_BAND * (steps - k) as f32 / steps as f32;
            eye.update(view, eye_at(y), dt, |_| Some(WATER));
            assert_eq!(
                eye.active_volume(),
                None,
                "{shape}: the tint never triggers"
            );
            if k < steps {
                assert_eq!(eye.fog_weight(), 1.0, "{shape}, y = {y}");
            }
            proximities.push(fog_proximity(eye_at(y), face, Vec3::Y));
        }
        assert!((proximities[0] - 1.0).abs() < 1e-5, "{proximities:?}");
        assert_eq!(proximities.last(), Some(&0.0));
        for pair in proximities.windows(2) {
            assert!(pair[1] <= pair[0], "{shape}: monotone {pair:?}");
        }
    }

    let bob = dev_head_bob();
    let full = bob.speed_threshold + crate::view_feel::BOB_EASE_IN_BAND;
    for (shape, volumes) in pools() {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        for fps in [30.0, 60.0, 144.0] {
            let mut eye = FluidEye::default();
            let mut previous: Option<(f32, f32, f32)> = None;
            let mut below = 0;
            for offset in bob_offsets(&bob, full, fps, 10.0) {
                let y = SURFACE_Y + offset;
                eye.update(view, eye_at(y), 1.0 / fps, |_| Some(WATER));
                assert_eq!(eye.active_volume(), None, "{shape}: the bob never triggers");
                let proximity = fog_proximity(eye_at(y), face, Vec3::Y);
                let factor = eye.fog_weight() * proximity;
                if y < SURFACE_Y {
                    below += 1;
                    assert_eq!(proximity, 0.0, "{shape}, y = {y}");
                }
                if let Some((last_y, last_proximity, last_factor)) = previous {
                    let bound = (y - last_y).abs() / FLUID_FOG_BAND + 1e-4;
                    assert!(
                        (proximity - last_proximity).abs() <= bound,
                        "{shape}, {fps} fps: proximity {last_proximity} -> {proximity}"
                    );
                    assert!(
                        (factor - last_factor).abs() <= bound,
                        "{shape}, {fps} fps: fog factor {last_factor} -> {factor}"
                    );
                }
                previous = Some((y, proximity, factor));
            }
            assert!(below > 0, "{shape}: the bob crosses the plane");
        }
    }
}

// The weight rides every presented frame beside the fluid view tint, from the
// same eye update, and unload writes the dry weight back.
#[test]
fn frame_fog_weight_is_sent_beside_the_view_tint_every_frame_and_reset_at_unload() {
    let frame_loop = include_str!("../frame_loop/mod.rs");
    let tint = "set_fluid_view_tint(";
    let weight = "set_fluid_fog_weight(";
    assert_eq!(
        frame_loop.matches(tint).count(),
        1,
        "one per-frame tint write"
    );
    assert_eq!(
        frame_loop.matches(weight).count(),
        1,
        "one per-frame weight write"
    );
    // Both writes sit in one block: a few lines apart, no brace between.
    let (first, second) = {
        let (a, b) = (
            frame_loop.find(tint).unwrap(),
            frame_loop.find(weight).unwrap(),
        );
        (a.min(b), a.max(b))
    };
    let between = &frame_loop[first..second];
    assert!(
        between.matches('\n').count() <= 4 && !between.contains(['{', '}']),
        "the weight is written beside the tint"
    );

    let presentation = include_str!("mod.rs");
    let start = presentation
        .find("fn clear_fluid_eye_presentation(")
        .expect("unload clears the eye presentation");
    let body = &presentation[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("renderer.set_fluid_fog_weight(self.fluid_eye.fog_weight());"),
        "unload sends the reset weight"
    );

    let mut app = test_app();
    app.fluid_eye = {
        let volumes = pool();
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
        let mut eye = FluidEye::default();
        for _ in 0..30 {
            eye.update(view, eye_at(SURFACE_Y - 1.0), 1.0 / 60.0, |_| Some(WATER));
        }
        eye
    };
    assert_eq!(app.fluid_eye.fog_weight(), 0.0);
    app.clear_fluid_eye_presentation();
    assert_eq!(app.fluid_eye.fog_weight(), 1.0, "the dry weight");
}

/// content/dev's water as its manifest declares it: `fogDistance` 3,
/// `fogColor` [0.16, 0.22, 0.19], `wavePeriod` 7.875, amplitude and length at
/// their defaults.
fn dev_water() -> FluidDescriptor {
    FluidDescriptor {
        view_tint: [0.1, 0.3, 0.4, 0.35],
        fog_distance: Some(3.0),
        fog_color: Some([0.16, 0.22, 0.19]),
        wave_period: Some(7.875),
        ..descriptor("water", 0.75)
    }
}

// The local declaration maps to the packed row the shader reads: fog
// extinction, the murk albedo, and the wave. An omitted fog colour follows
// the view tint, so a reload changing only the tint changes the row.
#[test]
fn dev_waters_declaration_packs_its_fog_and_wave_and_an_omitted_fog_colour_follows_the_tint() {
    assert_eq!(
        surface_look(&dev_water()),
        FluidSurfaceLook {
            surface_opacity: 0.75,
            fog_distance: Some(3.0),
            fog_color: [0.16, 0.22, 0.19],
            wave_period: Some(7.875),
            wave_amplitude: 8.0,
            wave_length: [64.0, 80.0],
        }
    );
    let row = surface_look(&dev_water()).pack();
    assert_eq!(row.surface, [0.75, 1.0, 0.0, 0.0]);
    for (packed, colour) in row.fog_albedo.iter().zip([0.16, 0.22, 0.19]) {
        assert!(
            (packed - colour * 0.25).abs() < 1e-7,
            "{:?}",
            row.fog_albedo
        );
    }
    assert_eq!(row.wave, [1.0 / 7.875, 8.0, 64.0, 80.0]);

    let clear = FluidDescriptor {
        fog_distance: None,
        fog_color: None,
        ..dev_water()
    };
    let look = surface_look(&clear);
    assert_eq!(look.fog_color, [0.1, 0.3, 0.4], "fogColor follows viewTint");
    let row = look.pack();
    assert_eq!(row.fog_extinction(), 0.0);

    let retinted = FluidDescriptor {
        view_tint: [0.5, 0.1, 0.2, 0.35],
        ..clear.clone()
    };
    let changed = surface_look(&retinted).pack();
    assert_ne!(
        changed, row,
        "a tint-only reload re-uploads the fluid's slots"
    );
    assert_eq!(changed.surface, row.surface);
    assert_eq!(changed.wave, row.wave);
    for (packed, colour) in changed.fog_albedo.iter().zip([0.5, 0.1, 0.2]) {
        assert!((packed - colour * 0.25).abs() < 1e-7);
    }
    // A declared fog colour does not follow the tint.
    let declared = FluidDescriptor {
        view_tint: [0.5, 0.1, 0.2, 0.35],
        ..dev_water()
    };
    assert_eq!(
        surface_look(&declared).pack(),
        surface_look(&dev_water()).pack()
    );
}

// A fluid with `surfaceOpacity` 0 and `fogDistance` 1 draws its visible
// volumes, for the murk alone; with fog omitted it draws nothing.
#[test]
fn a_zero_opacity_fluid_draws_only_when_it_declares_fog() {
    for (fog_distance, expected) in [(Some(1.0), 1), (None, 0)] {
        let mut app = test_app();
        let water = FluidDescriptor {
            fog_distance,
            ..descriptor("water", 0.0)
        };
        script_ctx(&app).data_registry.borrow_mut().fluids =
            FluidRegistry::from_declarations("test", [Ok(water)]);
        let mut world = level_world("pool", 1);
        world.environment_volumes = EnvironmentVolumesSection {
            level_gravity: LEVEL_GRAVITY,
            volumes: vec![fluid_record("water", 0.0)],
        };
        install_environment(&mut app, world);
        let local = script_ctx(&app).data_registry.borrow().fluids.clone();
        assert_eq!(local.descriptors().len(), 1, "the declaration validates");
        let draws = collect_fluid_surface_draws(
            &mut app.fluid_surface_render,
            app.level.as_ref().expect("installed"),
            &VisibleCells::DrawAll,
            Vec3::ZERO,
            &app.environment_volumes,
            &app.environment_fluids,
            &local,
        )
        .to_vec();
        assert_eq!(draws.len(), expected, "fog {fog_distance:?}: {draws:?}");
        if let Some(draw) = draws.first() {
            assert_eq!(draw.params.opacity(), 0.0);
            assert_eq!(draw.params.fog_extinction(), 3.0);
        }
    }
}
