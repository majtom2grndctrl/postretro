// Real-map portal orientation report: which tier of `LevelWorld::portal_planes`
// orients each baked portal, how the whole-box bounds rule differs from the
// old bounds-centre rule, and whether the map's locator agrees.
// See: context/lib/rendering_pipeline.md §2
//
// Ignored. Reads `.prl` paths from POSTRETRO_WALK_SWEEP_PRL, as the sweep does;
// `real_map_sweep` prints the same lines per map. Report only: nothing fails.
//
//   POSTRETRO_WALK_SWEEP_PRL=content/dev/maps/campaign-test.prl \
//     cargo test --release -p postretro-visibility real_map_portal_orientation_report \
//     -- --ignored --nocapture --test-threads=1

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use super::real_map_sweep::{load_world, map_name, sweep_paths};

/// `portal_planes.rs`'s side-evidence threshold, mirrored: the old rule is
/// rebuilt here, so the builder keeps one behaviour.
const SIDE_EVIDENCE_EPSILON: f32 = 1e-3;
/// Locator probe offset from a portal's centroid along its oriented normal.
const PROBE_OFFSET: f32 = 1e-3;

/// Side evidence: +1 / -1 for the Newell normal's positive / negative side,
/// 0 for none.
type Side = f32;

#[derive(Debug, Default)]
struct OrientationCounts {
    portals: usize,
    means_tier: usize,
    bounds_tier: usize,
    unoriented: usize,
    /// Old and new rules orient the portal opposite ways.
    flipped: usize,
    old_oriented_new_unoriented: usize,
    old_unoriented_new_oriented: usize,
    /// Oriented portals whose locator probes were checked.
    probed: usize,
    /// A probe landed in the portal's other cell.
    probe_mismatch: usize,
    /// No mismatch, but a probe landed in neither of the portal's cells.
    probe_third_cell: usize,
    /// Flipped portals whose probes both agree with the new / the old
    /// orientation, or with neither.
    flipped_probe_new: usize,
    flipped_probe_old: usize,
    flipped_probe_neither: usize,
    /// Portals where this file's means tier disagrees with the builder's:
    /// nonzero means the mirror below has drifted from `portal_planes.rs`.
    means_mirror_drift: usize,
}

/// The builder's tier rule: both cells' evidence for one tier. `None` passes
/// to the next tier; `Some(0)` is a disagreement, which leaves the portal
/// unoriented.
fn decide(front: Side, back: Side) -> Option<Side> {
    if front == 0.0 && back == 0.0 {
        return None;
    }
    if front != 0.0 && front == back {
        return Some(0.0);
    }
    Some(if front != 0.0 { front } else { -back })
}

fn polygon_centroid(polygon: &[Vec3]) -> Vec3 {
    if polygon.is_empty() {
        return Vec3::ZERO;
    }
    polygon.iter().copied().sum::<Vec3>() / polygon.len() as f32
}

/// `portal_planes.rs`'s Newell normal, mirrored.
fn newell_normal(polygon: &[Vec3]) -> Vec3 {
    let n = polygon.len();
    if n < 3 {
        return Vec3::ZERO;
    }
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = polygon[i];
        let nxt = polygon[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-12 {
        return Vec3::ZERO;
    }
    normal.normalize()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Ahead locates to the front cell and behind to the back cell.
    Agrees,
    /// A probe lands in the portal's other cell.
    Mismatch,
    /// No mismatch, but a probe lands in neither of the portal's cells.
    ThirdCell,
}

/// Locate a point `PROBE_OFFSET` ahead of `centroid` along `normal` and one
/// behind it, against the portal's front and back cells.
fn probe(world: &LevelWorld, centroid: Vec3, normal: Vec3, front: usize, back: usize) -> Probe {
    let ahead = world.locate_cell(centroid + normal * PROBE_OFFSET);
    let behind = world.locate_cell(centroid - normal * PROBE_OFFSET);
    if ahead == back || behind == front {
        Probe::Mismatch
    } else if ahead == front && behind == back {
        Probe::Agrees
    } else {
        Probe::ThirdCell
    }
}

fn count(world: &LevelWorld) -> OrientationCounts {
    let planes = world.portal_planes();
    let centroids: Vec<Vec3> = world
        .portals
        .iter()
        .map(|p| polygon_centroid(&p.polygon))
        .collect();
    let means: Vec<Option<Vec3>> = world
        .cells
        .iter()
        .map(|cell| {
            let start = cell.portal_ref_start as usize;
            let end = start + cell.portal_ref_count as usize;
            let refs = world.cell_portal_refs.get(start..end)?;
            let found: Vec<Vec3> = refs
                .iter()
                .filter_map(|&p| centroids.get(p as usize).copied())
                .collect();
            (!found.is_empty()).then(|| found.iter().sum::<Vec3>() / found.len() as f32)
        })
        .collect();
    let centres: Vec<Vec3> = world
        .cells
        .iter()
        .map(|c| (c.bounds_min + c.bounds_max) * 0.5)
        .collect();

    let mut out = OrientationCounts {
        portals: world.portals.len(),
        ..OrientationCounts::default()
    };
    for (i, portal) in world.portals.iter().enumerate() {
        let plane = planes[i];
        let normal = newell_normal(&portal.polygon);
        let centroid = centroids[i];
        let dist = -normal.dot(centroid);
        let side = |point: Option<Vec3>| -> Side {
            let Some(point) = point else { return 0.0 };
            let s = normal.dot(point) + dist;
            if s.abs() > SIDE_EVIDENCE_EPSILON {
                s.signum()
            } else {
                0.0
            }
        };
        let (front, back) = (portal.front_cell, portal.back_cell);
        let new_sign: Side = if plane.oriented {
            plane.normal.dot(normal).signum()
        } else {
            0.0
        };
        let old_sign = if normal == Vec3::ZERO {
            0.0
        } else {
            let means_tier = decide(
                side(means.get(front).copied().flatten()),
                side(means.get(back).copied().flatten()),
            );
            if let Some(sign) = means_tier {
                if sign != new_sign {
                    out.means_mirror_drift += 1;
                }
                if sign != 0.0 {
                    out.means_tier += 1;
                }
                sign
            } else {
                if plane.oriented {
                    out.bounds_tier += 1;
                }
                decide(
                    side(centres.get(front).copied()),
                    side(centres.get(back).copied()),
                )
                .unwrap_or(0.0)
            }
        };
        if !plane.oriented {
            out.unoriented += 1;
        }
        match (old_sign != 0.0, new_sign != 0.0) {
            (true, true) if old_sign != new_sign => out.flipped += 1,
            (true, false) => out.old_oriented_new_unoriented += 1,
            (false, true) => out.old_unoriented_new_oriented += 1,
            _ => {}
        }
        if !plane.oriented {
            continue;
        }
        out.probed += 1;
        let new_probe = probe(world, centroid, plane.normal, front, back);
        match new_probe {
            Probe::Mismatch => out.probe_mismatch += 1,
            Probe::ThirdCell => out.probe_third_cell += 1,
            Probe::Agrees => {}
        }
        if old_sign != 0.0 && old_sign != new_sign {
            let new_agrees = new_probe == Probe::Agrees;
            let old_agrees = probe(world, centroid, -plane.normal, front, back) == Probe::Agrees;
            match (new_agrees, old_agrees) {
                (true, _) => out.flipped_probe_new += 1,
                (false, true) => out.flipped_probe_old += 1,
                (false, false) => out.flipped_probe_neither += 1,
            }
        }
    }
    out
}

/// Print one map's orientation counts, (a)–(f).
pub(super) fn report_orientation(name: &str, world: &LevelWorld) {
    let c = count(world);
    eprintln!(
        "[orientation] {name}: (a) portals={} (b) means_tier={} (c) bounds_tier={} \
         (d) unoriented={} (e) old_rule_differs={} [flipped={} \
         old_oriented_new_unoriented={} old_unoriented_new_oriented={}] \
         (f) of {} oriented: probe_mismatch={} probe_third_cell={}; \
         flipped probes agree new/old/neither={}/{}/{}; means_mirror_drift={}",
        c.portals,
        c.means_tier,
        c.bounds_tier,
        c.unoriented,
        c.flipped + c.old_oriented_new_unoriented + c.old_unoriented_new_oriented,
        c.flipped,
        c.old_oriented_new_unoriented,
        c.old_unoriented_new_oriented,
        c.probed,
        c.probe_mismatch,
        c.probe_third_cell,
        c.flipped_probe_new,
        c.flipped_probe_old,
        c.flipped_probe_neither,
        c.means_mirror_drift,
    );
}

#[test]
#[ignore = "manual orientation report over baked PRLs named in POSTRETRO_WALK_SWEEP_PRL"]
fn real_map_portal_orientation_report() {
    for path in sweep_paths() {
        let (world, _) = load_world(&path);
        report_orientation(&map_name(&path), &world);
    }
}
