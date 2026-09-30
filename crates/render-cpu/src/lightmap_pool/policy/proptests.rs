// Property tests over random target, cap and ready sequences, executed on the GPU mirror.

use proptest::prelude::*;

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};

use super::gpu_mirror::GpuMirror;
use super::*;

const EDGE: u32 = 64;
const ALIGN: u32 = 4;
const BLOCKS: usize = 14;

#[derive(Debug, Clone)]
struct Step {
    cap: u32,
    /// `(block, class)`: class 0 untargets, 1 mandatory, 2 visible, 3 band.
    changes: Vec<(u32, u8, u32)>,
    /// Bit `b` makes targeted, non-resident block `b` ready.
    ready: u32,
    release: bool,
    fail: Option<usize>,
    abort: bool,
}

fn step() -> impl Strategy<Value = Step> {
    (
        0u32..=4,
        prop::collection::vec((0..BLOCKS as u32, 0u8..4, 0u32..8), 0..6),
        any::<u32>(),
        prop::bool::weighted(0.3),
        prop::option::weighted(0.15, 0usize..8),
        prop::bool::weighted(0.1),
    )
        .prop_map(|(cap, changes, ready, release, fail, abort)| Step {
            cap,
            changes,
            ready,
            release,
            fail,
            abort,
        })
}

fn class_of(code: u8) -> Option<LightmapBlockClass> {
    match code {
        1 => Some(LightmapBlockClass::Mandatory),
        2 => Some(LightmapBlockClass::Visible),
        3 => Some(LightmapBlockClass::Band),
        _ => None,
    }
}

fn never_refused(class: Option<LightmapBlockClass>) -> bool {
    matches!(
        class,
        Some(LightmapBlockClass::Mandatory | LightmapBlockClass::Visible)
    )
}

fn assert_state(model: &LightmapPoolModel, classes: &[Option<LightmapBlockClass>]) {
    let rects: Vec<(u32, Slot)> = model
        .resident_blocks()
        .iter()
        .map(|&block| (block, model.slots[block as usize].unwrap()))
        .collect();
    for (i, &(a, sa)) in rects.iter().enumerate() {
        assert!(
            classes[a as usize].is_some(),
            "untargeted block {a} resident"
        );
        assert!(sa.layer < model.layers());
        assert!(sa.x + sa.width <= EDGE && sa.y + sa.height <= EDGE);
        assert_eq!((sa.x % ALIGN, sa.y % ALIGN), (0, 0));
        if classes[a as usize] == Some(LightmapBlockClass::Band) {
            assert!(sa.layer < model.cap_eff(), "band block {a} past the cap");
        }
        for &(b, sb) in &rects[i + 1..] {
            let apart = sa.layer != sb.layer
                || sa.x + sa.width <= sb.x
                || sb.x + sb.width <= sa.x
                || sa.y + sa.height <= sb.y
                || sb.y + sb.height <= sa.y;
            assert!(apart, "blocks {a} and {b} overlap: {sa:?} {sb:?}");
        }
    }
}

fn run(extents: Vec<(u32, u32)>, cap: u32, steps: Vec<Step>) {
    let mut model = LightmapPoolModel::new(extents, ALIGN, EDGE, cap).unwrap();
    let mut gpu = GpuMirror::new(&model);
    let mut classes: Vec<Option<LightmapBlockClass>> = vec![None; BLOCKS];
    for step in steps {
        if step.release {
            model.release_retirement();
        }
        let mut next = classes.clone();
        let mut leads = [0u32; BLOCKS];
        for &(block, code, lead) in &step.changes {
            next[block as usize] = class_of(code);
            leads[block as usize] = lead;
        }
        let changed: Vec<u32> = (0..BLOCKS as u32)
            .filter(|&b| step.changes.iter().any(|c| c.0 == b))
            .collect();
        let set: Vec<LightmapTarget> = changed
            .iter()
            .filter_map(|&block| {
                next[block as usize].map(|class| LightmapTarget {
                    block,
                    class,
                    lead: leads[block as usize],
                })
            })
            .collect();
        let remove: Vec<u32> = changed
            .iter()
            .copied()
            .filter(|&b| next[b as usize].is_none())
            .collect();
        let ready: Vec<u32> = (0..BLOCKS as u32)
            .filter(|&b| {
                step.ready & (1 << b) != 0 && next[b as usize].is_some() && !model.is_resident(b)
            })
            .collect();
        let resident_before: Vec<bool> = (0..BLOCKS as u32).map(|b| model.is_resident(b)).collect();
        let retiring_before = model.retiring();
        let allocations_before = model.texture_allocations();

        let plan = model.plan_drain(DrainRequest {
            pool_cap_layers: step.cap,
            target_reset: None,
            target_set: &set,
            target_remove: &remove,
            ready: &ready,
        });
        for &block in &ready {
            if never_refused(next[block as usize]) {
                assert!(
                    !plan.refused.contains(&block),
                    "never-refused {block} refused"
                );
                assert!(
                    plan.installed.contains(&block) || plan.deferred.contains(&block),
                    "never-refused {block} neither installed nor deferred"
                );
            }
        }
        assert!(
            plan.deferred.is_empty() || retiring_before,
            "deferred without a retirement"
        );
        for eviction in &plan.evicted {
            let class = next[eviction.block as usize];
            assert!(
                !never_refused(class),
                "{eviction:?} evicted a {class:?} block"
            );
            assert_eq!(
                eviction.reason == EvictionReason::Untargeted,
                class.is_none()
            );
        }
        if plan.growth.is_some() {
            assert!(!retiring_before, "growth while retiring");
        }
        assert!(!(plan.report.repacked && plan.growth.is_some()));
        let grew = plan.growth.is_some();
        assert_eq!(
            model.texture_allocations(),
            allocations_before + u32::from(grew),
            "only growth allocates a texture"
        );

        if step.abort {
            model.abort_drain();
            assert_eq!(model.retiring(), retiring_before);
            assert_eq!(model.texture_allocations(), allocations_before);
            gpu.assert_matches(&model);
            continue;
        }
        if let Some(pick) = step.fail
            && !model.plan().uploads.is_empty()
        {
            let uploads = &model.plan().uploads;
            let block = uploads[pick % uploads.len()].block;
            model.fail_install(block).unwrap();
        }
        classes = next;
        gpu.execute(model.plan());
        gpu.assert_matches(&model);
        assert_state(&model, &classes);
        for block in 0..BLOCKS {
            if resident_before[block] && never_refused(classes[block]) {
                assert!(
                    model.is_resident(block as u32),
                    "never-refused {block} lost"
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(384))]

    #[test]
    fn placement_never_refuses_or_evicts_mandatory_or_visible_and_every_entry_samples_its_block(
        extents in prop::collection::vec((1u32..=64, 1u32..=64), BLOCKS),
        cap in 0u32..=4,
        steps in prop::collection::vec(step(), 1..24),
    ) {
        run(extents, cap, steps);
    }

    #[test]
    fn placement_holds_its_invariants_with_small_blocks_under_heavy_churn(
        extents in prop::collection::vec((1u32..=24, 1u32..=24), BLOCKS),
        cap in 0u32..=2,
        steps in prop::collection::vec(step(), 1..32),
    ) {
        run(extents, cap, steps);
    }
}
