// The per-pose memo's key: what each pose kind bakes from, and nothing
// else, so a `light_poses` edit keeps the termini and a domain change keeps
// them too while re-keying every mid pose.

use std::sync::Arc;

use super::*;
use crate::fixture_pipeline::{fixture_path, load_map};
use crate::mover_probe_channel::posed_solid::PosedMovers;
use crate::mover_probe_channel::{NearFieldDomain, PosedScene};

/// The static patches every key below reads as the domain's.
const PATCHES: [u32; 3] = [4, 5, 6];

// On the closet, the door's poses keyed under one id-34 volume key. A
// terminus key ignores the near-field domain, the pose count, the pose's
// index and the mover id, so the door's termini key alike as its only two
// poses, under other domain probes or patches and under another id. A mid
// pose's key folds the domain's probes and, re-baking a later bounce posed,
// its static patches, so another list re-keys every one; at one posed
// bounce or none it traces no later bounce, and another patch list keeps it. Every pose re-keys under
// another bounce count, round-trip bar or id-34 volume key.
#[test]
fn a_pose_key_folds_what_its_kind_bakes_from_and_nothing_else() {
    let fixture = load_map(&fixture_path("mover-closet"));
    let poses = fixture.placed_mover_poses(1.0);
    let posed = PosedMovers::resolve(
        &poses,
        &fixture.kinematic_movers,
        &fixture.geometry.texture_names,
    )
    .expect("the closet's door poses");
    let (analysed, solids) = (&poses.analysed[0], &posed.movers()[0]);
    let count = solids.solids.len();
    assert!(count >= 3, "the door bakes mid poses: {count}");
    let job = |mover_id: u32, domain_probes: &[u32], keep: &[usize]| MoverBounceJob {
        mover_id,
        label: analysed.label.clone(),
        bounces: 5,
        high_bounces: 5,
        scenes: keep
            .iter()
            .enumerate()
            .map(|(pose, &solid)| {
                PosedScene::new(Arc::clone(&solids.solids[solid]), pose).expect("posed sides")
            })
            .collect(),
        knots: keep.iter().map(|&solid| analysed.knots[solid]).collect(),
        domain: NearFieldDomain::new(analysed, 1.0),
        domain_probes: domain_probes.to_vec(),
    };
    let every: Vec<usize> = (0..count).collect();
    let cache_dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(cache_dir.path()).expect("cache");
    let volume = CacheKey::new("sh_base_volume", 1, b"volume");
    let memo = PoseMemo::new(&cache, &volume, false);
    let key_over = |memo: &PoseMemo<'_>, job: &MoverBounceJob, pose, bounces, patches: &[u32]| {
        memo.key(job, pose, bounces, None, patches).as_filename()
    };
    let key = |job: &MoverBounceJob, pose, bounces| key_over(&memo, job, pose, bounces, &PATCHES);

    let full = job(7, &[10, 11], &every);
    let termini = job(7, &[10, 11], &[0, count - 1]);
    let wider = job(7, &[10, 11, 12], &every);
    let other_id = job(8, &[10, 11], &every);
    for (terminus, pose) in [(0, 0), (1, count - 1)] {
        assert_eq!(
            key(&termini, terminus, 5),
            key(&full, pose, 5),
            "pose count"
        );
        assert_eq!(key(&wider, pose, 5), key(&full, pose, 5), "domain probes");
        assert_eq!(
            key_over(&memo, &full, pose, 5, &[4, 5]),
            key(&full, pose, 5),
            "domain patches"
        );
    }
    let other_volume = PoseMemo::new(
        &cache,
        &CacheKey::new("sh_base_volume", 1, b"another volume"),
        false,
    );
    let exact_bar = PoseMemo::new(&cache, &volume, true);
    for pose in 0..count {
        assert_eq!(key(&other_id, pose, 5), key(&full, pose, 5), "mover id");
        assert_ne!(key(&full, pose, 2), key(&full, pose, 5), "bounce count");
        assert_ne!(
            key_over(&other_volume, &full, pose, 5, &PATCHES),
            key(&full, pose, 5),
            "volume key"
        );
        assert_ne!(
            key_over(&exact_bar, &full, pose, 5, &PATCHES),
            key(&full, pose, 5),
            "round-trip bar"
        );
    }
    for pose in 1..count - 1 {
        assert_ne!(
            key(&wider, pose, 5),
            key(&full, pose, 5),
            "a mid pose's probes"
        );
        assert_ne!(
            key_over(&memo, &full, pose, 5, &[4, 5]),
            key(&full, pose, 5),
            "a mid pose's patches"
        );
        assert_ne!(
            key_over(&memo, &full, pose, 2, &[4, 5]),
            key(&full, pose, 2),
            "a mid pose tracing one later bounce posed reads its patches"
        );
        assert_eq!(
            key_over(&memo, &full, pose, 1, &[4, 5]),
            key(&full, pose, 1),
            "a mid pose re-baking bounce one alone traces no patch"
        );
        assert_eq!(
            key_over(&memo, &full, pose, 0, &[4, 5]),
            key(&full, pose, 0),
            "a depth-only mid pose traces no patch"
        );
    }
    assert_ne!(key(&full, 0, 5), key(&full, count - 1, 5), "posed solids");
}
