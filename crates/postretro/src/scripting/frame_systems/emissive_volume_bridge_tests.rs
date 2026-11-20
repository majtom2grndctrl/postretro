// Emissive volume bridge: level population, frame-N packing, the dirty gate,
// the allocation-free frame path, and brightness animation on the script
// clock.
// See: context/lib/rendering_pipeline.md §7.3 · scripting.md §10.2a

use super::*;
use crate::alloc_probe::AllocSnapshot;
use postretro_entities::ComponentKind;
use postretro_level_format::emissive::MAX_EMISSIVE_VOLUMES;
use postretro_render_cpu::emissive_volume::{EMISSIVE_TABLE_ENTRY_BYTES, NO_VOLUME_FACTOR};
use postretro_render_cpu::flash_limiter::{LimiterFrameInput, pack_limiter_frame};

const DT: f32 = 1.0 / 60.0;

/// A presented frame of `dt` seconds with the limiter on.
fn limited(dt: f32) -> LimiterFrameUniform {
    pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        true,
        false,
    )
}

fn record(min: f32, max: f32, color: [f32; 3], faces: Vec<u32>, tag: &str) -> EmissiveVolumeRecord {
    EmissiveVolumeRecord {
        min,
        max,
        color,
        origin: [1.0, 2.0, 3.0],
        faces,
        tags: vec![tag.to_owned()],
    }
}

fn two_volumes() -> Vec<EmissiveVolumeRecord> {
    vec![
        record(0.5, 4.0, [1.0, 0.5, 0.25], vec![3], "bar_sign"),
        // Emptied by a smaller volume at compile time: no member face.
        record(0.0, 1.0, [1.0; 3], Vec::new(), "status_lamp"),
    ]
}

/// Every record over one white `_e` texture of strength 1, for the flash
/// limiter.
fn white_sources(records: &[EmissiveVolumeRecord]) -> Vec<EmissiveFlashSource> {
    vec![EmissiveFlashSource::new(1.0, [1.0; 3]); records.len()]
}

fn populated(records: &[EmissiveVolumeRecord]) -> (EntityRegistry, EmissiveVolumeBridge) {
    let mut registry = EntityRegistry::new();
    let mut bridge = EmissiveVolumeBridge::new();
    bridge.populate_from_level(&mut registry, records, &white_sources(records));
    (registry, bridge)
}

/// The packed factor of record `index`, as this frame's upload carries it.
fn packed(bridge: &mut EmissiveVolumeBridge, index: usize) -> [f32; 3] {
    let upload = bridge.pending_upload();
    let at = (index + 1) * EMISSIVE_TABLE_ENTRY_BYTES;
    std::array::from_fn(|lane| {
        let bytes = &upload.table[at + lane * 4..at + lane * 4 + 4];
        f32::from_le_bytes(bytes.try_into().unwrap())
    })
}

/// Pending entry runs as `(start, end)` pairs.
fn spans(bridge: &mut EmissiveVolumeBridge) -> Vec<(u32, u32)> {
    bridge
        .pending_upload()
        .runs
        .iter()
        .map(|run| (run.start, run.end))
        .collect()
}

fn close(a: [f32; 3], b: [f32; 3]) {
    for (x, y) in a.into_iter().zip(b) {
        assert!((x - y).abs() < 1e-6, "{a:?} vs {b:?}");
    }
}

fn write(
    registry: &mut EntityRegistry,
    id: EntityId,
    edit: impl FnOnce(&mut EmissiveVolumeComponent),
) {
    let mut next = registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap()
        .clone();
    edit(&mut next);
    registry.set_component(id, next).unwrap();
}

// One entity per record, in record order, emptied volumes included; each at
// its no-script state with the record's tags and origin.
#[test]
fn populate_spawns_one_authored_entity_per_record_in_order() {
    let records = two_volumes();
    let (registry, bridge) = populated(&records);
    for (index, record) in records.iter().enumerate() {
        let id = bridge
            .entity_for_map_index(index)
            .expect("every record spawns");
        let component = registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap();
        assert_eq!(
            component,
            &EmissiveVolumeComponent::authored(record.min, record.max, record.color)
        );
        assert_eq!(registry.get_tags(id).unwrap(), record.tags.as_slice());
        assert_eq!(
            registry.get_component::<Transform>(id).unwrap().position,
            Vec3::from(record.origin)
        );
    }
    assert_eq!(bridge.entity_for_map_index(2), None);
    assert_eq!(
        registry
            .iter_with_kind(ComponentKind::EmissiveVolume)
            .count(),
        2
    );
}

// A3 runtime half: with no script, a volume presents max × its authored
// colour; brightness 0 and 0.5 walk the authored range.
#[test]
fn no_script_packs_max_times_colour_and_brightness_walks_the_range() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    bridge.update(&mut registry, 0.0, None);
    close(packed(&mut bridge, 0), [4.0, 2.0, 1.0]);
    close(packed(&mut bridge, 1), [1.0; 3]);

    let id = bridge.entity_for_map_index(0).unwrap();
    for (brightness, expected) in [(0.0, [0.5, 0.25, 0.125]), (0.5, [2.25, 1.125, 0.5625])] {
        write(&mut registry, id, |volume| volume.brightness = brightness);
        bridge.update(&mut registry, 0.0, None);
        close(packed(&mut bridge, 0), expected);
    }
}

// P10 and P49 at the pack: whatever the component holds, brightness and
// colour channels clamp to [0, 1].
#[test]
fn the_pack_clamps_component_brightness_and_colour() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| {
        volume.brightness = 1.5;
        volume.color = [100.0, -1.0, f32::NAN];
    });
    bridge.update(&mut registry, 0.0, None);
    close(packed(&mut bridge, 0), [4.0, 0.0, 0.0]);
    write(&mut registry, id, |volume| {
        volume.brightness = -0.5;
        volume.color = [1.0; 3];
    });
    bridge.update(&mut registry, 0.0, None);
    close(packed(&mut bridge, 0), [0.5; 3]);
}

// P12: a component write drained in frame N, before the bridge step, lands
// in frame N's packed table and pending upload.
#[test]
fn a_write_before_the_bridge_step_lands_in_that_frames_upload() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    bridge.update(&mut registry, 0.0, None);
    bridge.mark_uploaded();

    // Frame N: the drain writes a dimmer brightness and a red tint, then the
    // bridge runs before render.
    let id = bridge.entity_for_map_index(1).unwrap();
    write(&mut registry, id, |volume| {
        volume.brightness = 0.25;
        volume.color = [1.0, 0.05, 0.02];
    });
    bridge.update(&mut registry, 0.0, None);
    assert_eq!(spans(&mut bridge), [(2, 3)], "volume 1 is entry 2");
    close(packed(&mut bridge, 1), [0.25, 0.0125, 0.005]);
}

// Ms3: a static frame uploads nothing; one changed volume uploads exactly
// its one entry.
#[test]
fn a_static_frame_uploads_nothing_and_one_change_uploads_one_entry() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    bridge.update(&mut registry, 0.0, None);
    assert!(
        bridge.pending_upload().byte_count() > 0,
        "a fresh level sends its table once"
    );
    bridge.mark_uploaded();

    for _ in 0..3 {
        bridge.update(&mut registry, 0.0, None);
        let upload = bridge.pending_upload();
        assert!(upload.runs.is_empty());
        assert_eq!(upload.byte_count(), 0);
    }

    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| volume.color = [0.0, 1.0, 0.0]);
    bridge.update(&mut registry, 0.0, None);
    assert_eq!(
        bridge.pending_upload().byte_count(),
        EMISSIVE_TABLE_ENTRY_BYTES
    );

    // A renderer that could not take the writes leaves them pending.
    bridge.update(&mut registry, 0.0, None);
    assert_eq!(spans(&mut bridge), [(1, 2)]);
    bridge.mark_uploaded();
    bridge.update(&mut registry, 0.0, None);
    assert_eq!(bridge.pending_upload().byte_count(), 0);
}

// P36 at the table: the 256th volume is entry 256, distinct from entry 0.
#[test]
fn the_last_volume_a_map_may_carry_packs_its_own_entry() {
    let records: Vec<_> = (0..MAX_EMISSIVE_VOLUMES)
        .map(|_| record(0.0, 1.0, [1.0; 3], Vec::new(), "v"))
        .collect();
    let (mut registry, mut bridge) = populated(&records);
    bridge.update(&mut registry, 0.0, None);
    bridge.mark_uploaded();
    let last = bridge
        .entity_for_map_index(MAX_EMISSIVE_VOLUMES - 1)
        .unwrap();
    write(&mut registry, last, |volume| volume.brightness = 0.0);
    bridge.update(&mut registry, 0.0, None);
    assert_eq!(spans(&mut bridge), [(256, 257)]);
    close(packed(&mut bridge, MAX_EMISSIVE_VOLUMES - 1), [0.0; 3]);
    let entry_zero: [f32; 3] = std::array::from_fn(|lane| {
        f32::from_le_bytes(
            bridge.pending_upload().table[lane * 4..lane * 4 + 4]
                .try_into()
                .unwrap(),
        )
    });
    close(entry_zero, NO_VOLUME_FACTOR);
}

// Teardown clears everything; the next map starts fresh at brightness 1 and
// its authored colour, and re-sends the whole table.
#[test]
fn a_new_map_starts_fresh_after_clear() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| volume.brightness = 0.0);
    bridge.update(&mut registry, 0.0, None);
    bridge.mark_uploaded();

    bridge.clear();
    assert_eq!(bridge.entity_for_map_index(0), None);
    let mut next_registry = EntityRegistry::new();
    let next = [record(1.0, 2.0, [0.0, 0.0, 1.0], vec![0], "next")];
    bridge.populate_from_level(&mut next_registry, &next, &white_sources(&next));
    let id = bridge.entity_for_map_index(0).unwrap();
    assert_eq!(
        next_registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap(),
        &EmissiveVolumeComponent::authored(1.0, 2.0, [0.0, 0.0, 1.0])
    );
    bridge.update(&mut next_registry, 0.0, None);
    let upload = bridge.pending_upload();
    assert_eq!(upload.runs.len(), 1, "the whole table pends once");
    assert_eq!(upload.runs[0].start, 0);
    close(packed(&mut bridge, 0), [0.0, 0.0, 2.0]);
}

// The per-frame pack, the flash limiter and the upload view allocate nothing,
// changed or not.
#[test]
fn the_frame_path_allocates_nothing() {
    let control = AllocSnapshot::arm();
    std::hint::black_box(Vec::<u8>::with_capacity(32));
    assert!(
        control.allocs_since() > 0,
        "the allocation probe must be active"
    );

    let records: Vec<_> = (0..MAX_EMISSIVE_VOLUMES)
        .map(|_| record(0.0, 1.0, [1.0; 3], vec![], "v"))
        .collect();
    let (mut registry, mut bridge) = populated(&records);
    let frame = limited(DT);
    bridge.update(&mut registry, 0.0, Some(&frame));
    bridge.finish_presentation(true);
    bridge.mark_uploaded();
    for index in [3, 4, 200] {
        let id = bridge.entity_for_map_index(index).unwrap();
        write(&mut registry, id, |volume| volume.brightness = 0.5);
    }

    let probe = AllocSnapshot::arm();
    // The step to 0.5 is rate-capped: still moving on the next frame.
    bridge.update(&mut registry, 0.0, Some(&frame));
    bridge.finish_presentation(true);
    let bytes = bridge.pending_upload().byte_count();
    bridge.mark_uploaded();
    // A failed acquire rolls back; the frame after takes its time.
    bridge.update(&mut registry, 0.0, Some(&frame));
    bridge.finish_presentation(false);
    for _ in 0..30 {
        bridge.update(&mut registry, 0.0, Some(&frame));
        bridge.finish_presentation(true);
    }
    bridge.mark_uploaded();
    bridge.update(&mut registry, 0.0, Some(&frame));
    bridge.finish_presentation(true);
    let static_bytes = bridge.pending_upload().byte_count();
    assert_eq!(probe.allocs_since(), 0);
    assert_eq!(bytes, 3 * EMISSIVE_TABLE_ENTRY_BYTES);
    assert_eq!(static_bytes, 0, "a settled frame uploads nothing");
}

// P12 in the frame: the bridge packs and uploads after the script drain and
// before the render, once per frame, in the gameplay redraw.
#[test]
fn the_frame_packs_emissive_volumes_between_the_drain_and_the_render() {
    // Whitespace is stripped from both sides so rustfmt line breaks never move
    // a needle out of reach.
    let frame_loop: String = include_str!("../../frame_loop/mod.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let at = |needle: &str| {
        let needle: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(frame_loop.matches(&needle).count(), 1, "one `{needle}`");
        frame_loop.find(&needle).unwrap()
    };
    let drain = at("cpu_stages.scope(cpu_timing::FrameStage::ScriptDrain)");
    let update = at("bridge.update(&mut registry, app.script_time, Some(&limiter));");
    let upload = at("renderer.upload_emissive_volume_table(upload)");
    let render = at("renderer.render_frame_indirect(");
    assert!(drain < update && update < upload && upload < render);
    assert!(
        frame_loop[update..upload].contains("bridge.pending_upload()"),
        "the upload sends this frame's pending entries"
    );
    // The flash limiter settles once the renderer knows whether the frame
    // presented (P17), beside the ghost clamps.
    let settle = at("session.emissive_volume_bridge.finish_presentation(presented);");
    let ghosts = at("app.ghost_render.finish_presentation(presented);");
    assert!(render < settle && render < ghosts);
}

// ---------------------------------------------------------------------------
// Brightness animation on the script clock (`research.md` ordering pins).
// ---------------------------------------------------------------------------

use postretro_entities::EmissiveAnimation;

fn curve(period_ms: f32, play_count: Option<u32>, brightness: Vec<f32>) -> EmissiveAnimation {
    EmissiveAnimation {
        period_ms,
        play_count,
        brightness,
    }
}

/// A 1 s looping ramp 0 → 1: its sample at `t` seconds into a run is `t`
/// (mod 1), so a sample names its run's start.
fn ramp() -> EmissiveAnimation {
    curve(1000.0, None, vec![0.0, 1.0])
}

fn brightness_of(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap()
        .brightness
}

fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "{a} vs {b}");
}

/// One volume with `animation` installed, its bridge at script time 0.
fn animating(animation: EmissiveAnimation) -> (EntityRegistry, EmissiveVolumeBridge, EntityId) {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| {
        volume.install_animation(animation)
    });
    bridge.update(&mut registry, 0.0, None);
    (registry, bridge, id)
}

// The bridge writes each frame's sample into the component and packs it.
#[test]
fn a_running_animation_writes_its_sample_into_brightness_and_the_pack() {
    let (mut registry, mut bridge, id) = animating(ramp());
    approx(brightness_of(&registry, id), 0.0);
    bridge.update(&mut registry, 0.25, None);
    approx(brightness_of(&registry, id), 0.25);
    // Volume 0: min 0.5, max 4, colour (1, 0.5, 0.25); factor at 0.25 is 1.375.
    close(packed(&mut bridge, 0), [1.375, 0.6875, 0.34375]);
    bridge.update(&mut registry, 1.75, None);
    approx(brightness_of(&registry, id), 0.75);
}

// levelLoad installs start at script time 0, wherever the first frame lands.
#[test]
fn installed_animations_start_at_script_time_zero() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| volume.install_animation(ramp()));
    bridge.start_installed_animations(&registry);
    bridge.update(&mut registry, 0.4, None);
    approx(brightness_of(&registry, id), 0.4);
}

// P1 and P4 at the bridge: a brightness write clears a finite fade mid-run;
// the value holds and the fade's settle never writes over it.
#[test]
fn a_brightness_write_mid_fade_holds_past_the_fades_end() {
    let (mut registry, mut bridge, id) = animating(curve(1000.0, Some(1), vec![1.0, 0.0]));
    bridge.update(&mut registry, 0.5, None);
    write(&mut registry, id, |volume| volume.hold_brightness(0.5));
    for t in [0.6, 1.0, 1.5, 3.0] {
        bridge.update(&mut registry, t, None);
        approx(brightness_of(&registry, id), 0.5);
    }
}

// P5: clearing the curve holds the last sampled brightness.
#[test]
fn clearing_the_animation_holds_the_last_sample() {
    let (mut registry, mut bridge, id) = animating(ramp());
    bridge.update(&mut registry, 0.3, None);
    write(&mut registry, id, |volume| volume.animation = None);
    for t in [0.4, 0.9, 2.0] {
        bridge.update(&mut registry, t, None);
        approx(brightness_of(&registry, id), 0.3);
    }
}

// P6: re-installing the running curve keeps its start; after a finite run
// settles, the same install restarts from the first sample.
#[test]
fn an_identical_reinstall_keeps_its_start_until_a_finite_run_settles() {
    let (mut registry, mut bridge, id) = animating(ramp());
    bridge.update(&mut registry, 0.3, None);
    write(&mut registry, id, |volume| volume.install_animation(ramp()));
    bridge.update(&mut registry, 0.4, None);
    approx(brightness_of(&registry, id), 0.4);

    let once = curve(1000.0, Some(1), vec![0.0, 1.0]);
    write(&mut registry, id, |volume| {
        volume.install_animation(once.clone())
    });
    bridge.update(&mut registry, 0.5, None); // a new curve: starts at 0.5
    bridge.update(&mut registry, 1.2, None);
    approx(brightness_of(&registry, id), 0.7);
    write(&mut registry, id, |volume| {
        volume.install_animation(once.clone())
    });
    bridge.update(&mut registry, 1.3, None);
    approx(brightness_of(&registry, id), 0.8); // still the 0.5 start
    bridge.update(&mut registry, 1.6, None); // past one period: settles on 1
    let settled = registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap();
    assert_eq!(
        (settled.brightness, settled.animation.as_ref()),
        (1.0, None)
    );

    write(&mut registry, id, |volume| {
        volume.install_animation(once.clone())
    });
    bridge.update(&mut registry, 2.0, None);
    approx(brightness_of(&registry, id), 0.0); // restarted at 2.0
    bridge.update(&mut registry, 2.25, None);
    approx(brightness_of(&registry, id), 0.25);
}

// P30: a brightness write and an identical re-install between two bridge
// frames (one drain) restart the curve, although the bridge never sees the
// cleared state.
#[test]
fn a_clear_and_identical_reinstall_in_one_drain_restarts_the_curve() {
    let (mut registry, mut bridge, id) = animating(ramp());
    bridge.update(&mut registry, 0.6, None);
    approx(brightness_of(&registry, id), 0.6);
    write(&mut registry, id, |volume| {
        volume.hold_brightness(0.0);
        volume.install_animation(ramp());
    });
    bridge.update(&mut registry, 0.7, None);
    approx(brightness_of(&registry, id), 0.0);
    bridge.update(&mut registry, 0.95, None);
    approx(brightness_of(&registry, id), 0.25);
}

// P44: a colour write mid-flicker keeps the run's start and samples.
#[test]
fn a_colour_write_keeps_the_running_curves_start_and_samples() {
    let (mut registry, mut bridge, id) = animating(ramp());
    bridge.update(&mut registry, 0.2, None);
    write(&mut registry, id, |volume| volume.color = [1.0, 0.05, 0.02]);
    bridge.update(&mut registry, 0.45, None);
    approx(brightness_of(&registry, id), 0.45);
    // Factor at 0.45: 0.5 + 0.45 × 3.5 = 2.075, in the new colour.
    close(packed(&mut bridge, 0), [2.075, 0.10375, 0.0415]);
}

// P8 and P9 at the bridge: a finite count plays that many periods, then
// holds the last sample; a flat fade settles on its one value and holds it.
#[test]
fn finite_runs_settle_on_their_last_sample_and_hold() {
    let (mut registry, mut bridge, id) = animating(curve(500.0, Some(2), vec![0.2, 0.8]));
    bridge.update(&mut registry, 0.75, None);
    assert!(
        registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap()
            .animation
            .is_some(),
        "the second period is still playing"
    );
    bridge.update(&mut registry, 1.0, None);
    let settled = registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap();
    assert_eq!(
        (settled.brightness, settled.animation.as_ref()),
        (0.8, None)
    );

    let (mut registry, mut bridge, id) = animating(curve(400.0, Some(1), vec![0.6; 16]));
    for t in [0.1, 0.3, 0.4, 1.0, 5.0] {
        bridge.update(&mut registry, t, None);
        approx(brightness_of(&registry, id), 0.6);
    }
    assert!(
        registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap()
            .animation
            .is_none()
    );
}

// P13: while the script clock is frozen the sample holds.
#[test]
fn a_frozen_script_clock_holds_the_sample() {
    let (mut registry, mut bridge, id) = animating(ramp());
    for _ in 0..5 {
        bridge.update(&mut registry, 0.35, None);
        approx(brightness_of(&registry, id), 0.35);
    }
}

// The animating frame path allocates nothing once a run has started.
#[test]
fn an_animating_frame_allocates_nothing() {
    let (mut registry, mut bridge, _) = animating(ramp());
    bridge.update(&mut registry, 0.1, None);
    bridge.mark_uploaded();

    let probe = AllocSnapshot::arm();
    for frame in 2..10 {
        bridge.update(&mut registry, f64::from(frame) * 0.05, None);
        let _ = bridge.pending_upload().byte_count();
        bridge.mark_uploaded();
    }
    assert_eq!(probe.allocs_since(), 0);
}

// ---------------------------------------------------------------------------
// Co-op seeding
// ---------------------------------------------------------------------------

// The binding identity: record index and authored origin, whatever moves the
// entity afterwards.
#[test]
fn binding_identity_is_the_record_index_and_its_authored_origin() {
    let (mut registry, bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(1).unwrap();
    registry
        .set_component(
            id,
            Transform {
                position: Vec3::splat(50.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        )
        .unwrap();
    assert_eq!(bridge.map_index_for_entity(id), Some(1));
    assert_eq!(
        bridge.map_origin_for_entity(id),
        Some(Vec3::new(1.0, 2.0, 3.0))
    );
    assert_eq!(bridge.entity_for_map_index(2), None);
    let stranger = registry.spawn(Transform::default());
    assert_eq!(bridge.map_index_for_entity(stranger), None);
    assert_eq!(bridge.map_origin_for_entity(stranger), None);
}

// P21 at the bridge: a replicated run seeded at the host's rebased start
// survives the same frame's update and a later colour-only record (no start),
// and a start far enough back settles on the curve's last sample at once
// (P22). Mirrors fog's `replicated_fog_seed_keeps_age_through_same_frame_tick`.
#[test]
fn replicated_emissive_seed_keeps_age_through_same_frame_update_and_colour_record() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| {
        volume.install_animation(curve(5000.0, Some(1), vec![0.0, 1.0]))
    });
    assert!(bridge.seed_replicated_state(&registry, id, Some(8.0)));
    bridge.update(&mut registry, 10.0, None);
    approx(brightness_of(&registry, id), 0.4);

    write(&mut registry, id, |volume| volume.color = [1.0, 0.0, 0.0]);
    assert!(bridge.seed_replicated_state(&registry, id, None));
    bridge.update(&mut registry, 11.0, None);
    approx(brightness_of(&registry, id), 0.6);

    assert!(bridge.seed_replicated_state(&registry, id, Some(4.0)));
    bridge.update(&mut registry, 11.0, None);
    let settled = registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap();
    assert_eq!(
        (settled.brightness, settled.animation.as_ref()),
        (1.0, None)
    );
}

// A record that names no start leaves a new run to start at the next update;
// a held record drops the slot; a non-finite start or a foreign entity is
// refused.
#[test]
fn replicated_emissive_seed_without_start_is_lazy_and_bad_seeds_are_refused() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| volume.install_animation(ramp()));
    assert!(bridge.seed_replicated_state(&registry, id, None));
    bridge.update(&mut registry, 20.0, None);
    approx(brightness_of(&registry, id), 0.0);
    bridge.update(&mut registry, 20.25, None);
    approx(brightness_of(&registry, id), 0.25);

    write(&mut registry, id, |volume| volume.hold_brightness(0.5));
    assert!(bridge.seed_replicated_state(&registry, id, Some(1.0)));
    bridge.update(&mut registry, 21.0, None);
    approx(brightness_of(&registry, id), 0.5);

    assert!(!bridge.seed_replicated_state(&registry, id, Some(f64::NAN)));
    assert!(!bridge.seed_replicated_state(&registry, id, Some(f64::MAX)));
    let stranger = registry.spawn(Transform::default());
    assert!(!bridge.seed_replicated_state(&registry, stranger, None));
}

// ---------------------------------------------------------------------------
// Photosensitivity flash limiter (`research.md` P9, P13–P18, P33, P34, P52,
// P56). The clamp's own rules are proven in `render-cpu` `emissive_flash`;
// these prove the bridge feeds it every volume and keeps scripts unclamped.
// ---------------------------------------------------------------------------

use postretro_entities::SlotValue;
use postretro_level_format::emissive::{EmissiveSection, EmissiveTexture};
use postretro_render_cpu::flash_clamp::INTENSITY_RATE_PER_SECOND;

/// The example's `flicker` curve (`sdk/lib/entities/emissive_volumes.ts`).
const FLICKER_PATTERN: [f32; 8] = [0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65];
const STATUS_RED: [f32; 3] = [1.0, 0.05, 0.02];
const STATUS_GREEN: [f32; 3] = [0.02, 1.0, 0.05];
const STATUS_BLUE: [f32; 3] = [0.02, 0.05, 1.0];

/// `flicker({ min, max, rate })` as the SDK builds it.
fn flicker_curve(lo: f32, hi: f32, rate: f32) -> EmissiveAnimation {
    curve(
        1000.0 / rate,
        None,
        FLICKER_PATTERN.iter().map(|t| lo + t * (hi - lo)).collect(),
    )
}

/// One white sign: max 1, strength 1, white `_e`.
fn sign() -> (EntityRegistry, EmissiveVolumeBridge, EntityId) {
    let (registry, bridge) = populated(&[record(0.0, 1.0, [1.0; 3], vec![0], "sign")]);
    let id = bridge.entity_for_map_index(0).unwrap();
    (registry, bridge, id)
}

/// One presented frame at script time `t`, settled as presented.
fn present(
    bridge: &mut EmissiveVolumeBridge,
    registry: &mut EntityRegistry,
    t: f64,
    frame: &LimiterFrameUniform,
) -> [f32; 3] {
    bridge.update(registry, t, Some(frame));
    bridge.finish_presentation(true);
    let factor = packed(bridge, 0);
    bridge.mark_uploaded();
    factor
}

fn square(n: usize, a: f32, b: f32) -> f32 {
    if (n as f32 * DT * 6.0).fract() < 0.5 {
        a
    } else {
        b
    }
}

fn redness(rgb: [f32; 3]) -> f32 {
    let total = rgb[0] + rgb[1] + rgb[2];
    if total < 0.003 {
        return 0.0;
    }
    ((rgb[0] / total - 1.0 / 3.0) * 1.5).clamp(0.0, 1.0)
}

/// WCAG transitions of `series` (one value per frame) in any one second: a
/// 0.1 swing from the last extremum, its darker state under 0.8.
fn worst_transitions(series: &[f32]) -> usize {
    let mut counted_at = Vec::new();
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
            counted_at.push(n as f32 * DT);
        }
    }
    (0..counted_at.len())
        .map(|i| {
            counted_at[i..]
                .iter()
                .take_while(|&&t| t < counted_at[i] + 1.0)
                .count()
        })
        .max()
        .unwrap_or(0)
}

// P14: the example's rate-6 flicker presents at most 3 flashes a second
// with the limiter on, at no more than 4.0/s, while the component (what
// scripts and the wire read) samples the curve exactly as with no limiter.
#[test]
fn the_limited_flicker_presents_three_flashes_while_scripts_keep_the_curve() {
    let (mut registry, mut bridge, id) = sign();
    let (mut free_registry, mut free_bridge, free_id) = sign();
    for (registry, id) in [(&mut registry, id), (&mut free_registry, free_id)] {
        write(registry, id, |volume| {
            volume.install_animation(flicker_curve(0.3, 1.0, 6.0))
        });
    }
    let mut presented = Vec::new();
    let mut unlimited = Vec::new();
    for n in 0..120 {
        let t = f64::from(n as u32) / 60.0;
        presented.push(present(&mut bridge, &mut registry, t, &limited(DT))[0]);
        free_bridge.update(&mut free_registry, t, None);
        unlimited.push(packed(&mut free_bridge, 0)[0]);
        assert_eq!(
            brightness_of(&registry, id),
            brightness_of(&free_registry, free_id),
            "scripts read the unclamped sample"
        );
    }
    assert!(worst_transitions(&unlimited) > 6);
    assert!(worst_transitions(&presented) <= 6, "{presented:?}");
    for pair in presented.windows(2) {
        assert!((pair[1] - pair[0]).abs() <= INTENSITY_RATE_PER_SECOND * DT + 1e-5);
    }
}

// P34: static writes alternating 0 and 1 six times a second, each its own
// drain, meet the same bound; the component reads each written value.
#[test]
fn static_writes_present_limited_and_read_back_as_written() {
    let (mut registry, mut bridge, id) = sign();
    let presented: Vec<f32> = (0..120)
        .map(|n| {
            let b = square(n, 0.0, 1.0);
            write(&mut registry, id, |volume| volume.hold_brightness(b));
            let factor = present(&mut bridge, &mut registry, 0.0, &limited(DT));
            assert_eq!(brightness_of(&registry, id), b);
            factor[0]
        })
        .collect();
    assert!(worst_transitions(&presented) <= 6, "{presented:?}");
}

// P52 at the bridge: a held red onset presents desaturated while
// `component.color` reads the written red; a blue write presents blue on its
// own frame.
#[test]
fn a_held_red_reads_back_red_and_a_blue_write_presents_that_frame() {
    let (mut registry, mut bridge, id) = sign();
    let color = |n| {
        if square(n, 1.0, 0.0) == 1.0 {
            STATUS_RED
        } else {
            STATUS_GREEN
        }
    };
    let mut held = false;
    for n in 0..120 {
        write(&mut registry, id, |volume| volume.color = color(n));
        let factor = present(&mut bridge, &mut registry, 0.0, &limited(DT));
        let written = registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap()
            .color;
        assert_eq!(written, color(n), "scripts read the written colour");
        if written == STATUS_RED && redness(factor) < 0.7 {
            held = true;
            write(&mut registry, id, |volume| volume.color = STATUS_BLUE);
            assert_eq!(
                present(&mut bridge, &mut registry, 0.0, &limited(DT)),
                STATUS_BLUE
            );
            break;
        }
    }
    assert!(held, "the red-flash rule holds a red onset");
}

// P33: clearing the animation while the limiter holds a volume leaves
// brightness at the bridge's last scripted sample, and the presented glow
// eases toward it under the rate cap.
#[test]
fn a_clear_during_a_hold_eases_presented_glow_toward_the_last_sample() {
    let (mut registry, mut bridge, id) = sign();
    write(&mut registry, id, |volume| {
        volume.install_animation(flicker_curve(0.0, 1.0, 6.0))
    });
    let mut cleared_at = None;
    for n in 0..240 {
        let t = f64::from(n as u32) / 60.0;
        let factor = present(&mut bridge, &mut registry, t, &limited(DT));
        let scripted = brightness_of(&registry, id);
        if n > 60 && (factor[0] - scripted).abs() > 0.2 {
            cleared_at = Some((t, scripted, factor[0]));
            break;
        }
    }
    let (t, last_sample, mut previous) = cleared_at.expect("the limiter holds the flicker");
    write(&mut registry, id, |volume| volume.animation = None);
    for k in 1..=30 {
        let factor = present(
            &mut bridge,
            &mut registry,
            t + f64::from(k) / 60.0,
            &limited(DT),
        );
        assert_eq!(
            brightness_of(&registry, id),
            last_sample,
            "brightness holds"
        );
        assert!((factor[0] - previous).abs() <= INTENSITY_RATE_PER_SECOND * DT + 1e-5);
        previous = factor[0];
    }
    assert_eq!(
        previous, last_sample,
        "presented reaches the scripted sample"
    );
}

// P9 and P13: with the script clock frozen and a flat fade, presented-frame
// time still ages the clamp: nothing is counted, the glow presents as
// scripted, and a window the strobe spent before the freeze empties.
#[test]
fn a_frozen_flat_fade_presents_unchanged_and_the_clamp_keeps_aging() {
    let (mut registry, mut bridge, id) = sign();
    for n in 0..60 {
        let b = square(n, 0.0, 1.0);
        write(&mut registry, id, |volume| volume.hold_brightness(b));
        present(&mut bridge, &mut registry, 1.0, &limited(DT));
    }
    write(&mut registry, id, |volume| {
        volume.install_animation(curve(2000.0, Some(1), vec![0.0; 16]))
    });
    let frozen = 1.0;
    for n in 0..90 {
        let factor = present(&mut bridge, &mut registry, frozen, &limited(DT));
        assert_eq!(brightness_of(&registry, id), 0.0);
        if n >= 30 {
            assert_eq!(factor, [0.0; 3], "a flat fade presents unchanged");
        }
    }
    // The spent window aged out on presented time: a full rise is admitted.
    write(&mut registry, id, |volume| volume.hold_brightness(1.0));
    let rise: Vec<f32> = (0..20)
        .map(|_| present(&mut bridge, &mut registry, frozen, &limited(DT))[0])
        .collect();
    assert_eq!(rise.last(), Some(&1.0), "{rise:?}");
}

// P15 through the frame loop's limiter read: an explicit `false` in the
// player's setting presents a step in one frame; turning it back on
// presents that frame unchanged.
#[test]
fn the_players_setting_turns_the_limiter_off_and_on() {
    use crate::scripting_systems::ghost_render::ghost_limiter_frame;
    use postretro_render_cpu::flash_limiter::FLASH_LIMITER_SLOT;

    let (mut registry, mut bridge, id) = sign();
    let mut slots = postretro_entities::SlotTable::new();
    let set = |slots: &mut postretro_entities::SlotTable, on: bool| {
        slots
            .get_mut(FLASH_LIMITER_SLOT)
            .expect("the engine declares the limiter slot")
            .write_value(Some(SlotValue::Boolean(on)));
    };
    write(&mut registry, id, |volume| volume.hold_brightness(0.0));
    present(
        &mut bridge,
        &mut registry,
        0.0,
        &ghost_limiter_frame(&slots, DT),
    );
    write(&mut registry, id, |volume| volume.hold_brightness(1.0));
    let capped = present(
        &mut bridge,
        &mut registry,
        0.0,
        &ghost_limiter_frame(&slots, DT),
    );
    assert!(capped[0] < 1.0, "on by default: the step is rate-capped");

    set(&mut slots, false);
    write(&mut registry, id, |volume| volume.hold_brightness(0.0));
    let off = present(
        &mut bridge,
        &mut registry,
        0.0,
        &ghost_limiter_frame(&slots, DT),
    );
    assert_eq!(off, [0.0; 3], "off: a step lands in one frame");

    set(&mut slots, true);
    write(&mut registry, id, |volume| volume.hold_brightness(1.0));
    let enabling = present(
        &mut bridge,
        &mut registry,
        0.0,
        &ghost_limiter_frame(&slots, DT),
    );
    assert_eq!(enabling, [1.0; 3], "the enabling frame presents unchanged");
}

// P18: a level change starts every clamp fresh. A level loaded after a
// strobe spent the window presents its first frame unchanged.
#[test]
fn a_new_level_starts_every_clamp_fresh() {
    let (mut registry, mut bridge, id) = sign();
    for n in 0..60 {
        let b = square(n, 0.0, 1.0);
        write(&mut registry, id, |volume| volume.hold_brightness(b));
        present(&mut bridge, &mut registry, 0.0, &limited(DT));
    }
    write(&mut registry, id, |volume| volume.hold_brightness(0.0));
    for _ in 0..30 {
        present(&mut bridge, &mut registry, 0.0, &limited(DT));
    }
    bridge.clear();
    let mut next_registry = EntityRegistry::new();
    let next = [record(0.0, 1.0, [1.0; 3], vec![0], "next")];
    bridge.populate_from_level(&mut next_registry, &next, &white_sources(&next));
    assert_eq!(
        present(&mut bridge, &mut next_registry, 0.0, &limited(DT)),
        [1.0; 3]
    );
}

// P56 from the level data: a volume a later one emptied has no member face,
// so its source is dark and its flicker presents as scripted, never NaN.
#[test]
fn an_emptied_volume_presents_its_scripted_glow_under_the_limiter() {
    let records = [record(0.0, 1.0, [1.0; 3], Vec::new(), "emptied")];
    let section = EmissiveSection {
        textures: vec![EmissiveTexture {
            strength: 4.0,
            glow_color: [1.0; 3],
        }],
        volumes: records.to_vec(),
    };
    let sources = EmissiveFlashSource::for_volumes(&section, |_| Some(0));
    assert_eq!(sources, [EmissiveFlashSource::DARK]);
    let mut registry = EntityRegistry::new();
    let mut bridge = EmissiveVolumeBridge::new();
    bridge.populate_from_level(&mut registry, &records, &sources);
    let id = bridge.entity_for_map_index(0).unwrap();
    for n in 0..60 {
        let b = square(n, 0.0, 1.0);
        write(&mut registry, id, |volume| volume.hold_brightness(b));
        assert_eq!(
            present(&mut bridge, &mut registry, 0.0, &limited(DT)),
            [b; 3]
        );
    }
}
