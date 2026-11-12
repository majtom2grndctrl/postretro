// GPU proof that the per-mover knot bracket changes no lookup: `mc_interval`
// and `mb_entry_blend`, started from the bracket the real t′ block carries,
// return bit for bit what the linear knot scan they replaced returns, over
// many-knot, ramped, squeezed, on-knot and every-mid-pose-across cases.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The harness self-skips without an adapter and says so; set
// `POSTRETRO_REQUIRE_GPU` to make a missing adapter fail the test.
//
// Each (knot set, t′) pair is its own mover slot, so one dispatch reads every
// t′ through the per-frame path: `MoverTBlock::set_states` finds each slot's
// bracket and `take_upload` hands over the bytes both tables carry. Each
// slot then runs 64 probes, whose across and bounce marks grow denser with
// the probe index: probe 0 defines every mid pose, probe 63 none.

use postretro_level_format::mover_probe_channel::{
    MoverProbeChannelSection, MoverRecord, pose_interval,
};
use wgpu::util::DeviceExt;

use super::knots::MoverKnots;
use super::state::MoverTBlock;
use super::{MOVER_BOUNCE_WGSL, MOVER_CHANNEL_WGSL};
use crate::render::MoverTravel;
use crate::render::gpu_test_harness::gpu_or_skip;

const PROBES: u32 = 64;
const NONE: u32 = u32::MAX;
/// Words per (slot, probe) result: direct bracketed then linear (lo, hi,
/// weight bits), bounce bracketed then linear (affected, lo, hi, weight
/// bits).
const RESULT_WORDS: usize = 14;
/// Direct table header words (`MC_HEADER_*`) and bounce (`MB_HEADER_*`).
const MC_HEADER_WORDS: usize = 10;
const MB_HEADER_WORDS: usize = 8;
const MB_POSE_WORDS: usize = 5;

/// The linear scans the bracket replaced, verbatim but for their names.
const LINEAR_WGSL: &str = r#"
fn linear_mc_interval(run: u32, t: f32, across: u32, local_probe: u32) -> McInterval {
    let last = mover_channel[run + 1u] - 1u;
    if (t >= 1.0) {
        let pose = mc_knot_pose(run, last);
        return McInterval(pose, pose, 0.0);
    }
    var at = 0u;
    for (var knot = 1u; knot < last; knot = knot + 1u) {
        if (mc_knot_t(run, knot) > t) {
            break;
        }
        at = knot;
    }
    var lo = at;
    loop {
        if (lo == 0u || !mc_across(run, lo, across, local_probe)) {
            break;
        }
        lo = lo - 1u;
    }
    let lo_pose = mc_knot_pose(run, lo);
    let t_lo = mc_knot_t(run, lo);
    if (t <= t_lo) {
        return McInterval(lo_pose, lo_pose, 0.0);
    }
    var hi = at + 1u;
    loop {
        if (hi >= last || !mc_across(run, hi, across, local_probe)) {
            break;
        }
        hi = hi + 1u;
    }
    return McInterval(
        lo_pose,
        mc_knot_pose(run, hi),
        (t - t_lo) / (mc_knot_t(run, hi) - t_lo),
    );
}

fn linear_mb_entry_blend(cell: MbCellEntries, entry: u32, local_probe: u32, t: f32) -> MbBlend {
    if (!mb_mask_bit(mover_bounce[MB_HEADER_ENTRIES] + entry * MB_ENTRY_WORDS + MB_ENTRY_READ_MASK, local_probe).hit) {
        return MbBlend(false, MB_NONE, MB_NONE, 0.0);
    }
    let run = mover_bounce[mover_bounce[MB_HEADER_KNOTS] + mb_entry_word(entry, MB_ENTRY_SLOT)];
    let last = mover_bounce[run + 1u] - 1u;
    let poses = mb_entry_word(entry, MB_ENTRY_POSES);
    let first_half = cell.base_half + mb_entry_word(entry, MB_ENTRY_FIRST_HALF);
    if (t >= 1.0) {
        let state = mb_pose_state(poses, mover_bounce[run] - 1u, true, local_probe, first_half);
        return MbBlend(true, state, state, 0.0);
    }
    var at = 0u;
    for (var knot = 1u; knot < last; knot = knot + 1u) {
        if (mb_knot_t(run, knot) > t) {
            break;
        }
        at = knot;
    }
    var lo = at;
    var lo_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * lo], lo == 0u, local_probe, first_half);
    while (lo > 0u && lo_state == MB_ACROSS) {
        lo = lo - 1u;
        lo_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * lo], lo == 0u, local_probe, first_half);
    }
    let t_lo = mb_knot_t(run, lo);
    if (t <= t_lo) {
        return MbBlend(true, lo_state, lo_state, 0.0);
    }
    var hi = at + 1u;
    var hi_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * hi], hi == last, local_probe, first_half);
    while (hi < last && hi_state == MB_ACROSS) {
        hi = hi + 1u;
        hi_state = mb_pose_state(poses, mover_bounce[run + 3u + 2u * hi], hi == last, local_probe, first_half);
    }
    return MbBlend(true, lo_state, hi_state, (t - t_lo) / (mb_knot_t(run, hi) - t_lo));
}
"#;

const ENTRY_WGSL: &str = r#"
@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) probe: u32) {
    let slot = group.x;
    let out = (slot * 64u + probe) * 14u;
    let run = mover_channel[mover_channel[MC_HEADER_KNOTS] + slot];
    let across = across_runs[slot];
    let t = mc_mover_t(slot);
    let found = mc_interval(run, t, mc_mover_bracket(slot), across, probe);
    let linear = linear_mc_interval(run, t, across, probe);
    results[out] = found.lo;
    results[out + 1u] = found.hi;
    results[out + 2u] = bitcast<u32>(found.weight);
    results[out + 3u] = linear.lo;
    results[out + 4u] = linear.hi;
    results[out + 5u] = bitcast<u32>(linear.weight);
    let cell = MbCellEntries(0u, 0u, 0u);
    let bounce_t = mb_entry_t(slot);
    let blend = mb_entry_blend(cell, slot, probe, bounce_t);
    let scan = linear_mb_entry_blend(cell, slot, probe, bounce_t);
    results[out + 6u] = select(0u, 1u, blend.affected);
    results[out + 7u] = blend.lo;
    results[out + 8u] = blend.hi;
    results[out + 9u] = bitcast<u32>(blend.weight);
    results[out + 10u] = select(0u, 1u, scan.affected);
    results[out + 11u] = scan.lo;
    results[out + 12u] = scan.hi;
    results[out + 13u] = bitcast<u32>(scan.weight);
}
"#;

/// splitmix64: a seeded stream, so every run reads the same cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// `count` path knots: 0, then `count − 2` strictly ascending random
/// interior knots, then 1.
fn random_knots(rng: &mut Rng, count: usize) -> Vec<f32> {
    let mut interior: Vec<f32> = (0..count - 2).map(|_| rng.unit()).collect();
    interior.sort_by(f32::total_cmp);
    interior.dedup();
    interior.retain(|&t| t > 0.0);
    assert_eq!(interior.len(), count - 2, "distinct random knots");
    let mut knots = vec![0.0];
    knots.extend(interior);
    knots.push(1.0);
    knots
}

fn even_knots(count: usize) -> Vec<f32> {
    (0..count).map(|k| k as f32 / (count - 1) as f32).collect()
}

/// One slot: a mover record and the t′ the game hands over (`None`: no
/// state).
struct Slot {
    record: MoverRecord,
    t: Option<f32>,
}

/// Every case's knot set and ramp, each read at its knots' t′ exactly, one
/// f32 step either side, between neighbouring knots, at both ends and past
/// them, at a subnormal t′, with no state, and at random t′.
fn slots(rng: &mut Rng) -> Vec<Slot> {
    let cases: Vec<(Vec<f32>, (f32, f32))> = vec![
        (random_knots(rng, 70), (0.0, 1.0)),
        // Most of the 70 squeeze onto t′ 0 or 1.
        (random_knots(rng, 70), (0.3, 0.7)),
        (even_knots(65), (0.0, 1.0)),
        // MP1 and MP10: squeezed onto 0, then onto 1.
        (even_knots(6), (0.6, 0.9)),
        (even_knots(21), (0.6, 0.9)),
        (vec![0.0, 1.0], (0.0, 1.0)),
        (vec![0.0, 0.5, 1.0], (0.1, 0.2)),
        // A degenerate ramp reads t′ = t.
        (random_knots(rng, 64), (0.5, 0.5)),
    ];
    let mut slots = Vec::new();
    for (knots, ramp) in cases {
        let record = MoverRecord {
            mover_id: 0,
            ramp_start: ramp.0,
            ramp_end: ramp.1,
            knots,
        };
        let effective = record.effective_knots();
        let mut ts = vec![
            Some(0.0),
            Some(-0.0),
            Some(1.0),
            Some(1.5),
            Some(f32::MIN_POSITIVE / 4.0),
            Some(1.0f32.next_down()),
            None,
        ];
        for pair in effective.windows(2) {
            let (a, b) = (pair[0].t, pair[1].t);
            ts.extend([a, a.next_up(), b.next_down(), a + 0.5 * (b - a)].map(Some));
        }
        ts.extend((0..16).map(|_| Some(rng.unit())));
        slots.extend(ts.into_iter().map(|t| Slot {
            record: record.clone(),
            t,
        }));
    }
    for (slot, entry) in slots.iter_mut().enumerate() {
        entry.record.mover_id = slot as u32 + 1;
    }
    slots
}

/// What a probe defines at each pose of one slot: across (direct marks it,
/// or the bounce entry stores and reads nothing there), and the bounce
/// pose state.
#[derive(Clone, Copy, PartialEq)]
enum PoseState {
    Stored,
    Base,
    Across,
}

/// Probe `probe`'s state at mid pose: across with probability `probe / 63`,
/// otherwise stored or base evenly. A terminus is stored or reads the base.
fn pose_state(rng: &mut Rng, probe: u32, terminus: bool) -> PoseState {
    let roll = rng.unit();
    if !terminus && roll < probe as f32 / (PROBES - 1) as f32 {
        PoseState::Across
    } else if rng.next() & 1 == 0 {
        PoseState::Stored
    } else {
        PoseState::Base
    }
}

struct Tables {
    direct: Vec<u32>,
    bounce: Vec<u32>,
    /// Per slot: its direct across run's word, or `NONE`.
    across_runs: Vec<u32>,
    /// Per slot, per pose, per probe.
    states: Vec<Vec<[PoseState; PROBES as usize]>>,
    knots: MoverKnots,
}

fn tables(rng: &mut Rng, slots: &[Slot]) -> Tables {
    let section = MoverProbeChannelSection {
        mover_records: slots.iter().map(|slot| slot.record.clone()).collect(),
        ..super::tests::fixture_section([1, 1, 1], &[])
    };
    let ids: Vec<u32> = slots.iter().map(|slot| slot.record.mover_id).collect();
    let knots = MoverKnots::new(Some(&section), &ids).expect("every slot has a record");
    let mut block = MoverTBlock::new(&ids, &knots);
    let states: Vec<MoverTravel> = slots
        .iter()
        .filter_map(|slot| {
            slot.t.map(|t| MoverTravel {
                mover_id: slot.record.mover_id,
                t,
            })
        })
        .collect();
    block.set_states(&states);
    let t_block: Vec<u32> = block
        .take_upload()
        .expect("a handed-over state moves the block")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&word| u32::from_le_bytes(word))
        .collect();

    let states: Vec<Vec<[PoseState; PROBES as usize]>> = slots
        .iter()
        .map(|slot| {
            let poses = slot.record.knots.len();
            (0..poses)
                .map(|pose| {
                    let terminus = pose == 0 || pose + 1 == poses;
                    std::array::from_fn(|probe| pose_state(rng, probe as u32, terminus))
                })
                .collect()
        })
        .collect();
    let mask = |row: &[PoseState; PROBES as usize], want: PoseState| -> [u32; 2] {
        let bits = row
            .iter()
            .enumerate()
            .filter(|(_, state)| **state == want)
            .fold(0u64, |bits, (probe, _)| bits | 1 << probe);
        [bits as u32, (bits >> 32) as u32]
    };

    // Direct: header, knot block, across runs (one slot in five has none),
    // t′ block.
    let knots_word = MC_HEADER_WORDS;
    let mut direct = vec![0u32; knots_word + knots.table_word_count()];
    direct[9] = knots_word as u32;
    knots.write_table(&mut direct, knots_word);
    let mut across_runs = Vec::with_capacity(slots.len());
    for (slot, poses) in states.iter().enumerate() {
        if slot % 5 == 4 {
            across_runs.push(NONE);
            continue;
        }
        across_runs.push(direct.len() as u32);
        for row in &poses[1..poses.len() - 1] {
            direct.extend(mask(row, PoseState::Across));
        }
    }
    direct[5] = direct.len() as u32;
    direct.extend(&t_block);

    // Bounce: header, knot block, one entry per slot, pose blocks, t′
    // block. Probes with `probe % 7 == 3` are outside the read mask.
    let knots_word = MB_HEADER_WORDS;
    let mut bounce = vec![0u32; knots_word + knots.table_word_count()];
    bounce[7] = knots_word as u32;
    knots.write_table(&mut bounce, knots_word);
    let entries_word = bounce.len();
    bounce[5] = entries_word as u32;
    let read = (0..PROBES)
        .filter(|probe| probe % 7 != 3)
        .fold(0u64, |bits, probe| bits | 1 << probe);
    bounce.resize(entries_word + 5 * slots.len(), 0);
    for (slot, poses) in states.iter().enumerate() {
        let entry = entries_word + 5 * slot;
        let poses_word = bounce.len() as u32;
        bounce[entry..entry + 5].copy_from_slice(&[
            slot as u32,
            poses_word,
            read as u32,
            (read >> 32) as u32,
            7 * slot as u32,
        ]);
        for (pose, row) in poses.iter().enumerate() {
            bounce.extend(mask(row, PoseState::Stored));
            bounce.extend(mask(row, PoseState::Base));
            bounce.push(64 * pose as u32);
        }
        debug_assert_eq!(
            bounce.len() - poses_word as usize,
            poses.len() * MB_POSE_WORDS
        );
    }
    bounce[6] = bounce.len() as u32;
    bounce.extend(&t_block);

    Tables {
        direct,
        bounce,
        across_runs,
        states,
        knots,
    }
}

fn run(tables: &Tables, slots: u32) -> Vec<u32> {
    let Some(ctx) = gpu_or_skip("the_knot_bracket_reads_what_the_linear_knot_scan_read") else {
        return Vec::new();
    };
    let device = &ctx.device;
    let source = format!(
        "@group(0) @binding(0) var<storage, read> mover_channel: array<u32>;\n\
         @group(0) @binding(1) var<storage, read> mover_bounce: array<u32>;\n\
         @group(0) @binding(2) var<storage, read> across_runs: array<u32>;\n\
         @group(0) @binding(3) var<storage, read_write> results: array<u32>;\n\
         {MOVER_CHANNEL_WGSL}\n{MOVER_BOUNCE_WGSL}\n{LINEAR_WGSL}\n{ENTRY_WGSL}"
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("knot bracket harness shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("knot bracket harness"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let storage = |label: &str, words: &[u32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(words),
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let direct = storage("knot bracket direct table", &tables.direct);
    let bounce = storage("knot bracket bounce table", &tables.bounce);
    let across = storage("knot bracket across runs", &tables.across_runs);
    let size = (slots as usize * PROBES as usize * RESULT_WORDS * 4) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("knot bracket results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("knot bracket readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("knot bracket harness"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[(0, &direct), (1, &bounce), (2, &across), (3, &results)].map(
            |(binding, buffer)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            },
        ),
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("knot bracket harness"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("knot bracket harness"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(slots, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, size);
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map ok");
    let words = bytemuck::cast_slice(&slice.get_mapped_range().expect("mapped")).to_vec();
    readback.unmap();
    words
}

/// The bracket is a lookup acceleration only: for every t′, ramp, squeezed
/// knot and mark pattern, both shaders' lookups started from it return bit
/// for bit what their linear scans returned, and the direct lookup agrees
/// with the format's interval rule. The cases reach brackets past 60 knots,
/// walks of several across poses on both sides, t′ exactly on interior
/// knots, and probes whose every mid pose is blended across.
#[test]
fn the_knot_bracket_reads_what_the_linear_knot_scan_read() {
    let mut rng = Rng(0x005e_edb4_ace7);
    let slots = slots(&mut rng);
    let tables = tables(&mut rng, &slots);
    let results = run(&tables, slots.len() as u32);
    if results.is_empty() {
        return;
    }

    let (mut on_interior_knot, mut walked_down, mut walked_up, mut all_across) = (0, 0, 0, 0);
    let mut deepest_bracket = 0;
    for (slot, entry) in slots.iter().enumerate() {
        let knots = tables.knots.slot(slot);
        let last = knots.len() - 1;
        let stated = entry.t.map(|t| t.clamp(0.0, 1.0) + 0.0);
        for probe in 0..PROBES {
            let at = (slot * PROBES as usize + probe as usize) * RESULT_WORDS;
            let words = &results[at..at + RESULT_WORDS];
            let what = format!(
                "slot {slot} (t′ {stated:?}, {} knots), probe {probe}",
                knots.len()
            );
            assert_eq!(words[0..3], words[3..6], "direct: {what}");
            assert_eq!(words[6..10], words[10..14], "bounce: {what}");

            // The format's rule on the CPU keeps a subnormal t′, which this
            // GPU flushes to 0 in both lookups alike; the bracket is not
            // party to that.
            let Some(t) = stated.filter(|t| !t.is_subnormal()) else {
                continue;
            };
            let across = |pose: u32| {
                tables.across_runs[slot] != NONE
                    && tables.states[slot][pose as usize][probe as usize] == PoseState::Across
            };
            let expected = pose_interval(knots, t, |pose| !across(pose));
            assert_eq!(
                (words[0], words[1]),
                (expected.lo, expected.hi),
                "direct poses: {what}"
            );
            assert!(
                (f32::from_bits(words[2]) - expected.weight).abs() < 1.0e-6,
                "direct weight: {what}"
            );

            let bracket = tables.knots.bracket(slot, t) as usize;
            deepest_bracket = deepest_bracket.max(bracket);
            if t < 1.0 {
                on_interior_knot += usize::from(bracket > 0 && knots[bracket].t == t);
                walked_down += usize::from(knots[bracket].pose != expected.lo);
                walked_up +=
                    usize::from(expected.weight > 0.0 && knots[bracket + 1].pose != expected.hi);
            }
            all_across += usize::from(
                tables.across_runs[slot] != NONE
                    && last > 1
                    && knots[1..last].iter().all(|knot| across(knot.pose)),
            );
        }
    }
    assert!(
        deepest_bracket >= 60,
        "a many-knot bracket: {deepest_bracket}"
    );
    assert!(on_interior_knot > 0, "t′ exactly on an interior knot");
    assert!(
        walked_down > 0 && walked_up > 0,
        "walks across poses both ways"
    );
    assert!(
        all_across > 0,
        "a probe whose every mid pose is blended across"
    );
}
