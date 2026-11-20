// Emissive volume bridge: level population, frame-N packing, the dirty gate
// and the allocation-free frame path.
// See: context/lib/rendering_pipeline.md §7.3

use super::*;
use crate::alloc_probe::AllocSnapshot;
use postretro_entities::ComponentKind;
use postretro_level_format::emissive::MAX_EMISSIVE_VOLUMES;
use postretro_render_cpu::emissive_volume::{EMISSIVE_TABLE_ENTRY_BYTES, NO_VOLUME_FACTOR};

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

fn populated(records: &[EmissiveVolumeRecord]) -> (EntityRegistry, EmissiveVolumeBridge) {
    let mut registry = EntityRegistry::new();
    let mut bridge = EmissiveVolumeBridge::new();
    bridge.populate_from_level(&mut registry, records);
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
    bridge.update(&registry);
    close(packed(&mut bridge, 0), [4.0, 2.0, 1.0]);
    close(packed(&mut bridge, 1), [1.0; 3]);

    let id = bridge.entity_for_map_index(0).unwrap();
    for (brightness, expected) in [(0.0, [0.5, 0.25, 0.125]), (0.5, [2.25, 1.125, 0.5625])] {
        write(&mut registry, id, |volume| volume.brightness = brightness);
        bridge.update(&registry);
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
    bridge.update(&registry);
    close(packed(&mut bridge, 0), [4.0, 0.0, 0.0]);
    write(&mut registry, id, |volume| {
        volume.brightness = -0.5;
        volume.color = [1.0; 3];
    });
    bridge.update(&registry);
    close(packed(&mut bridge, 0), [0.5; 3]);
}

// P12: a component write drained in frame N, before the bridge step, lands
// in frame N's packed table and pending upload.
#[test]
fn a_write_before_the_bridge_step_lands_in_that_frames_upload() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    bridge.update(&registry);
    bridge.mark_uploaded();

    // Frame N: the drain writes a dimmer brightness and a red tint, then the
    // bridge runs before render.
    let id = bridge.entity_for_map_index(1).unwrap();
    write(&mut registry, id, |volume| {
        volume.brightness = 0.25;
        volume.color = [1.0, 0.05, 0.02];
    });
    bridge.update(&registry);
    assert_eq!(spans(&mut bridge), [(2, 3)], "volume 1 is entry 2");
    close(packed(&mut bridge, 1), [0.25, 0.0125, 0.005]);
}

// Ms3: a static frame uploads nothing; one changed volume uploads exactly
// its one entry.
#[test]
fn a_static_frame_uploads_nothing_and_one_change_uploads_one_entry() {
    let (mut registry, mut bridge) = populated(&two_volumes());
    bridge.update(&registry);
    assert!(
        bridge.pending_upload().byte_count() > 0,
        "a fresh level sends its table once"
    );
    bridge.mark_uploaded();

    for _ in 0..3 {
        bridge.update(&registry);
        let upload = bridge.pending_upload();
        assert!(upload.runs.is_empty());
        assert_eq!(upload.byte_count(), 0);
    }

    let id = bridge.entity_for_map_index(0).unwrap();
    write(&mut registry, id, |volume| volume.color = [0.0, 1.0, 0.0]);
    bridge.update(&registry);
    assert_eq!(
        bridge.pending_upload().byte_count(),
        EMISSIVE_TABLE_ENTRY_BYTES
    );

    // A renderer that could not take the writes leaves them pending.
    bridge.update(&registry);
    assert_eq!(spans(&mut bridge), [(1, 2)]);
    bridge.mark_uploaded();
    bridge.update(&registry);
    assert_eq!(bridge.pending_upload().byte_count(), 0);
}

// P36 at the table: the 256th volume is entry 256, distinct from entry 0.
#[test]
fn the_last_volume_a_map_may_carry_packs_its_own_entry() {
    let records: Vec<_> = (0..MAX_EMISSIVE_VOLUMES)
        .map(|_| record(0.0, 1.0, [1.0; 3], Vec::new(), "v"))
        .collect();
    let (mut registry, mut bridge) = populated(&records);
    bridge.update(&registry);
    bridge.mark_uploaded();
    let last = bridge
        .entity_for_map_index(MAX_EMISSIVE_VOLUMES - 1)
        .unwrap();
    write(&mut registry, last, |volume| volume.brightness = 0.0);
    bridge.update(&registry);
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
    bridge.update(&registry);
    bridge.mark_uploaded();

    bridge.clear();
    assert_eq!(bridge.entity_for_map_index(0), None);
    let mut next_registry = EntityRegistry::new();
    let next = [record(1.0, 2.0, [0.0, 0.0, 1.0], vec![0], "next")];
    bridge.populate_from_level(&mut next_registry, &next);
    let id = bridge.entity_for_map_index(0).unwrap();
    assert_eq!(
        next_registry
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap(),
        &EmissiveVolumeComponent::authored(1.0, 2.0, [0.0, 0.0, 1.0])
    );
    bridge.update(&next_registry);
    let upload = bridge.pending_upload();
    assert_eq!(upload.runs.len(), 1, "the whole table pends once");
    assert_eq!(upload.runs[0].start, 0);
    close(packed(&mut bridge, 0), [0.0, 0.0, 2.0]);
}

// The per-frame pack and its upload view allocate nothing, changed or not.
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
    bridge.update(&registry);
    bridge.mark_uploaded();
    for index in [3, 4, 200] {
        let id = bridge.entity_for_map_index(index).unwrap();
        write(&mut registry, id, |volume| volume.brightness = 0.5);
    }

    let probe = AllocSnapshot::arm();
    bridge.update(&registry);
    let bytes = bridge.pending_upload().byte_count();
    bridge.mark_uploaded();
    bridge.update(&registry);
    let static_bytes = bridge.pending_upload().byte_count();
    assert_eq!(probe.allocs_since(), 0);
    assert_eq!(bytes, 3 * EMISSIVE_TABLE_ENTRY_BYTES);
    assert_eq!(static_bytes, 0);
}

// P12 in the frame: the bridge packs and uploads after the script drain and
// before the render, once per frame, in the gameplay redraw.
#[test]
fn the_frame_packs_emissive_volumes_between_the_drain_and_the_render() {
    let frame_loop = include_str!("../../frame_loop/mod.rs");
    let at = |needle: &str| {
        assert_eq!(frame_loop.matches(needle).count(), 1, "one `{needle}`");
        frame_loop.find(needle).unwrap()
    };
    let drain = at("cpu_stages.scope(cpu_timing::FrameStage::ScriptDrain)");
    let update = at("bridge.update(&registry);");
    let upload = at("renderer.upload_emissive_volume_table(upload)");
    let render = at("renderer.render_frame_indirect(");
    assert!(drain < update && update < upload && upload < render);
    assert!(
        frame_loop[update..upload].contains("bridge.pending_upload()"),
        "the upload sends this frame's pending entries"
    );
}
