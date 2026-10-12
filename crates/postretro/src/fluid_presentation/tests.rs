// Camera-eye fluid presentation against head bob, the photosensitivity limiter
// and the mod's screen tint, plus the per-frame fluid surface draws.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md §Acceptance

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
use postretro_render_cpu::fluid_surface::FluidSurfaceRenderCollector;
use postretro_render_cpu::screen_effects::{EffectUniform, pack_effect_uniform};
use postretro_visibility::VisibleCells;

use super::eye::{EYE_FLUID_MARGIN, FLUID_PRESENTATION_RAMP_SECONDS, FluidEye, FluidLook};
use super::{collect_fluid_surface_draws, local_fluid_presentation};
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

// E24 P7: content/dev's bob stays inside the band, so treading at the surface
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

// E24 P7: an eye strobing across the band at 5 Hz presents at most three tint
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

// E24 P7: with the limiter off nothing is held, yet the bob still toggles at
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

// E24 P6: the fluid tint composes over the mod's tint, a script write while
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
    }
}

// Task 10 wiring: an installed level's fluid surfaces draw through the
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
    assert!((draws[0].surface_opacity - 0.6).abs() < 1e-6);

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
        |v| local_fluid_presentation(&volumes, &host_table, &local, v).map(|f| f.surface_opacity),
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
