// DIAGNOSTIC (task 5b): remove before landing.
//
// Probe dumps for the mid-travel poses' first-slice artifacts: per probe and
// pose, the enclosure test, domain membership, bounce marks, direct q per
// gated light and each pose's state sampled along the six axis normals; per
// t, the composed bounce and direct (the piecewise compose this module's
// parent mirrors) against a static-brush reference at t. Plus the later-share
// measurement for AC 19's probe groups. Output is JSON lines for the capture
// harness (`captures/t5b-harness/`).
//
//   POSTRETRO_DIAG_OUT=<path> cargo test -p postretro-level-compiler --bin prl-build \
//     diag_ -- --ignored --nocapture

use std::io::Write as _;

use glam::DVec3;
use rayon::prelude::*;

use super::*;
use crate::mover_probe_channel::NearFieldDomain;
use crate::mover_probe_channel::fixture_scene::SPACING;
use crate::sh_bake::vec3_from;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;

/// The six axis normals, in Quake axes: +x, −x, +y, −y, +z, −z.
const NORMALS: [[f64; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
];

/// Quake direction → engine direction (`quake_offset`'s inverse, unscaled).
fn engine_dir(q: [f64; 3]) -> [f32; 3] {
    [-q[1] as f32, q[2] as f32, -q[0] as f32]
}

/// Bilinear sample of a bordered 6×6 tile along engine direction `dir`, as
/// `sample_probe_atlas_slot` reads it.
fn sample_rgb(tile: &[[f32; 3]], dir: [f32; 3]) -> [f32; 3] {
    let dim = DEFAULT_IRRADIANCE_TILE_DIMENSION as usize;
    let [u, v] = irradiance_tile_texel_coord(
        dir,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    );
    let (x, y) = (u - 0.5, v - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| {
        let xi = (xi as isize).clamp(0, dim as isize - 1) as usize;
        let yi = (yi as isize).clamp(0, dim as isize - 1) as usize;
        tile[yi * dim + xi]
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    [0, 1, 2].map(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}

fn six(tile: &[OctahedralAtlasTexel]) -> Vec<[f32; 3]> {
    let rgb: Vec<[f32; 3]> = tile.iter().map(super::rgb).collect();
    NORMALS
        .iter()
        .map(|&n| sample_rgb(&rgb, engine_dir(n)))
        .collect()
}

fn fmt_six(values: &[[f32; 3]]) -> String {
    let parts: Vec<String> = values
        .iter()
        .map(|[r, g, b]| format!("[{r:.5},{g:.5},{b:.5}]"))
        .collect();
    format!("[{}]", parts.join(","))
}

/// A map baked as id 34 alone at bounce fill `fill`, with every mover absent
/// from the static world (or made static, for a `staticize_at` map).
fn base_at_fill(path: &Path, fill: f32) -> Reference {
    let scene = Scene::of(load_map(path));
    let base = with_ctx(&scene.fixture, |ctx, poses| {
        let plan = plan_half(&scene.fixture, ctx, poses, fill);
        bake_base(ctx, &plan, &MoverBounceRequest::default(), fill, None).1
    });
    let slots = compact_slots(&scene.layout);
    Reference { scene, base, slots }
}

/// `reference`'s bounce at the probe at `position`, matched by position
/// (static brushes can move the grid's bounds), where it is valid there.
fn bounce_at(reference: &Reference, position: DVec3) -> Option<Vec<OctahedralAtlasTexel>> {
    let layout = &reference.scene.layout;
    (0..layout.probe_positions.len())
        .find(|&r| (layout.probe_positions[r] - position).length() < 1.0e-4)
        .filter(|&r| layout.validity[r] != 0)
        .map(|r| reference.bounce(r))
}

struct Region {
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Region {
    fn contains(&self, q: [f64; 3]) -> bool {
        (0..3).all(|a| self.lo[a] <= q[a] && q[a] <= self.hi[a])
    }
}

fn out_file(name: &str) -> std::io::BufWriter<std::fs::File> {
    let dir = std::env::var("POSTRETRO_DIAG_OUT").expect("POSTRETRO_DIAG_OUT names a directory");
    std::fs::create_dir_all(&dir).expect("out dir");
    let path = Path::new(&dir).join(name);
    eprintln!("diag: writing {}", path.display());
    std::io::BufWriter::new(std::fs::File::create(path).expect("out file"))
}

/// Dump every probe in `region` (Quake units) for mover `mover` on `map`:
/// per pose and per t, as the header comment says.
fn dump_region(map: &str, mover: &str, region: Region, ts: &[f32], name: &str) {
    let text = std::fs::read_to_string(fixture_path(map)).expect("dev map");
    let dir = tempfile::tempdir().expect("maps dir");
    let write = |name: &str, text: &str| {
        let path = dir.path().join(format!("{name}.map"));
        std::fs::write(&path, text).expect("write map");
        path
    };
    let build = Build::bake(&write(map, &text), mover);
    // Today's compose: the same map with `light_poses 2` (brief AC 2).
    let named = format!("\"name\" \"{mover}\"");
    let termini = Build::bake(
        &write(
            &format!("{map}-termini"),
            &text.replace(&named, &format!("{named}\n\"light_poses\" \"2\"")),
        ),
        mover,
    );
    assert_eq!(termini.pose_count, 2);
    let analysed_all = build.scene.mover_poses();
    let analysed = analysed_all
        .analysed
        .iter()
        .find(|analysed| analysed.mover_id == build.mover_id)
        .expect("the mover");
    let domain = NearFieldDomain::new(analysed, SPACING);
    let posed = build.scene.poses();
    let solids = &posed
        .movers()
        .iter()
        .find(|posed| posed.mover_id == build.mover_id)
        .expect("posed")
        .solids;
    let layout = &build.scene.layout;
    let probes: Vec<usize> = (0..layout.probe_positions.len())
        .filter(|&p| region.contains(quake_offset(layout.probe_positions[p])))
        .collect();
    eprintln!("diag {map}: {} probes in region", probes.len());
    // Baked lights only: the fixture scene gates every map light, dynamic
    // ones included, which the real build never bakes into direct SH.
    let lights: Vec<_> = build
        .scene
        .static_channel_lights()
        .into_iter()
        .filter(|light| !light.authored.is_dynamic)
        .collect();
    // Every light's term at every region probe, traced once.
    let terms: Vec<Vec<[f32; 27]>> = probes
        .par_iter()
        .map(|&p| lights.iter().map(|light| build.scene.term(light, p)).collect())
        .collect();
    let first = &analysed.poses[0].bounds;
    let last = &analysed.poses[analysed.poses.len() - 1].bounds;
    // Reference at each t: id 34 at every bounce and every light's term,
    // with the mover's brushes static there.
    let references: Vec<(f32, Reference, Vec<Vec<[f32; 27]>>, Vec<Option<usize>>)> = ts
        .iter()
        .map(|&t| {
            let offset: DVec3 = (last.min - first.min) * f64::from(t);
            let path = write(
                &format!("{map}-{mover}-at-{t}"),
                &staticize_at(&text, mover, quake_offset(offset)),
            );
            let reference = Reference::bake(&path);
            // The static brushes can move the grid's bounds: match probes by
            // position, not index.
            let key = |q: [f64; 3]| q.map(|v| (v * 10.0).round() as i64);
            let by_position: HashMap<[i64; 3], usize> = (0..reference
                .scene
                .layout
                .probe_positions
                .len())
                .map(|r| (key(quake_offset(reference.scene.layout.probe_positions[r])), r))
                .collect();
            let matched: Vec<Option<usize>> = probes
                .iter()
                .map(|&p| {
                    by_position
                        .get(&key(quake_offset(layout.probe_positions[p])))
                        .copied()
                        .filter(|&r| reference.scene.layout.validity[r] != 0)
                })
                .collect();
            let rlights: Vec<_> = reference
                .scene
                .static_channel_lights()
                .into_iter()
                .filter(|light| !light.authored.is_dynamic)
                .collect();
            let rterms: Vec<Vec<[f32; 27]>> = matched
                .par_iter()
                .map(|r| match r {
                    None => Vec::new(),
                    Some(r) => rlights.iter().map(|light| reference.scene.term(light, *r)).collect(),
                })
                .collect();
            eprintln!(
                "diag {map}: reference at t {t} baked ({} of {} region probes matched valid)",
                matched.iter().flatten().count(),
                probes.len()
            );
            (t, reference, rterms, matched)
        })
        .collect();
    let mut out = out_file(name);
    writeln!(
        out,
        "{{\"kind\":\"header\",\"map\":\"{map}\",\"mover\":\"{mover}\",\"knots\":{:?},\"pose_count\":{},\"ts\":{:?},\"lights\":{:?},\"spacing_quake\":{}}}",
        build.knots.iter().map(|knot| knot.0).collect::<Vec<_>>(),
        build.pose_count,
        ts,
        lights.iter().map(|light| light.light_index).collect::<Vec<_>>(),
        f64::from(SPACING) * QUAKE_PER_METRE,
    )
    .unwrap();
    let sum_terms = |terms: &[[f32; 27]], vis: &dyn Fn(u32) -> f32| {
        let mut sum = [0f32; 27];
        for (light, term) in lights.iter().zip(terms) {
            let v = vis(light.light_index);
            for (total, value) in sum.iter_mut().zip(term) {
                *total += v * value;
            }
        }
        sh_tile(&sum)
    };
    for (row, &p) in probes.iter().enumerate() {
        let position = layout.probe_positions[p];
        let q = quake_offset(position);
        let point = vec3_from(position);
        let valid = layout.validity[p] != 0;
        let enclosed: Vec<bool> = solids.iter().map(|solid| solid.encloses(point)).collect();
        let mut line = format!(
            "{{\"kind\":\"probe\",\"p\":{p},\"q\":[{:.2},{:.2},{:.2}],\"valid\":{valid},\"domain\":{},\"enc\":{:?}",
            q[0],
            q[1],
            q[2],
            domain.contains(point),
            enclosed
        );
        if valid {
            // Bounce marks and each pose's state.
            let (cell, local) = build.local[&p];
            let view = build.section.bounce_entry_views().find(|view| {
                view.cell == cell
                    && view.entry.mover_id == build.mover_id
                    && view.entry.affected_probe_mask() & (1 << local) != 0
            });
            let base = build.base_tile(p);
            let marks: String = (0..build.pose_count)
                .map(|pose| match &view {
                    None => '-',
                    Some(view) => match view.entry.mark(pose, local) {
                        PoseMark::Stored => 'S',
                        PoseMark::Base => 'B',
                        PoseMark::Unmarked => '.',
                    },
                })
                .collect();
            let pose_bounce: Vec<String> = (0..build.pose_count)
                .map(|pose| {
                    let tile: Vec<OctahedralAtlasTexel> =
                        match view.as_ref().and_then(|view| view.pose_tile(pose, local)) {
                            Some(halves) => halves
                                .to_vec()
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .map(|texel| texel_from_halves(*texel))
                                .collect(),
                            None => base.clone(),
                        };
                    fmt_six(&six(&tile))
                })
                .collect();
            // q per gated light per pose.
            let bit = 1u64 << local;
            let mut qs = Vec::new();
            for record in build.section.direct_record_views().filter(|record| {
                record.cell == cell
                    && record.record.namespace == MoverLightNamespace::StaticLight
                    && record.record.probe_mask & bit != 0
            }) {
                let slot = (record.record.probe_mask & (bit - 1)).count_ones() as usize;
                for m in record.movers().filter(|m| m.mover_id == build.mover_id) {
                    qs.push(format!(
                        "\"{}\":{:?}",
                        record.record.light_index,
                        m.probe_q(slot)
                    ));
                }
            }
            // Each pose's direct.
            let pose_direct: Vec<String> = (0..build.pose_count)
                .map(|pose| {
                    let vis = |light: u32| -> f32 {
                        let mut v = 1.0;
                        for record in build.section.direct_record_views().filter(|record| {
                            record.cell == cell
                                && record.record.namespace == MoverLightNamespace::StaticLight
                                && record.record.probe_mask & bit != 0
                                && record.record.light_index == light
                        }) {
                            let slot =
                                (record.record.probe_mask & (bit - 1)).count_ones() as usize;
                            for m in record.movers().filter(|m| m.mover_id == build.mover_id) {
                                v = f32::from(m.probe_q(slot)[pose]) / 255.0;
                            }
                        }
                        v
                    };
                    fmt_six(&six(&sum_terms(&terms[row], &vis)))
                })
                .collect();
            line += &format!(
                ",\"marks\":\"{marks}\",\"q8\":{{{}}},\"pose_b\":[{}],\"pose_d\":[{}],\"base_b\":{}",
                qs.join(","),
                pose_bounce.join(","),
                pose_direct.join(","),
                fmt_six(&six(&base)),
            );
            // Composed at each t.
            let mut per_t = Vec::new();
            for (t, reference, rterms, matched) in &references {
                let bounce = build.bounce(p, *t);
                let visibility = build.visibility(p, *t);
                let vis = |light: u32| visibility.get(&light).copied().unwrap_or(1.0);
                let direct = sum_terms(&terms[row], &vis);
                let rvalid = matched[row].is_some();
                let (rb, rd) = if let Some(r) = matched[row] {
                    let rd = {
                        let mut sum = [0f32; 27];
                        for term in rterms[row].iter() {
                            for (total, value) in sum.iter_mut().zip(term) {
                                *total += value;
                            }
                        }
                        sh_tile(&sum)
                    };
                    (fmt_six(&six(&reference.bounce(r))), fmt_six(&six(&rd)))
                } else {
                    ("null".into(), "null".into())
                };
                per_t.push(format!(
                    "{{\"t\":{t},\"b\":{},\"d\":{},\"rvalid\":{rvalid},\"rb\":{rb},\"rd\":{rd}}}",
                    fmt_six(&six(&bounce)),
                    fmt_six(&six(&direct)),
                ));
            }
            line += &format!(",\"at\":[{}]", per_t.join(","));
            // Mover absent (the id-57-stripped frame) and today's compose.
            line += &format!(",\"free_d\":{}", fmt_six(&six(&sum_terms(&terms[row], &|_| 1.0))));
            let today: Vec<String> = references
                .iter()
                .map(|(t, ..)| {
                    let visibility = termini.visibility(p, *t);
                    let vis = |light: u32| visibility.get(&light).copied().unwrap_or(1.0);
                    format!(
                        "{{\"t\":{t},\"b\":{},\"d\":{}}}",
                        fmt_six(&six(&termini.bounce(p, *t))),
                        fmt_six(&six(&sum_terms(&terms[row], &vis)))
                    )
                })
                .collect();
            line += &format!(",\"today\":[{}]", today.join(","));
        }
        line += "}";
        writeln!(out, "{line}").unwrap();
    }
}

fn texel_from_halves(halves: [u16; 3]) -> OctahedralAtlasTexel {
    OctahedralAtlasTexel {
        rgba: [halves[0], halves[1], halves[2], f32_to_f16_bits(1.0)],
    }
}

#[test]
#[ignore = "DIAGNOSTIC (task 5b): dumps the rising room's probes for the interior luma pulse"]
fn diag_room_interior_probe_dump() {
    let ts: Vec<f32> = (0..=20).map(|i| i as f32 * 0.05).collect();
    dump_region(
        "mover-setpiece",
        "setpiece_room",
        Region {
            lo: [790.0, 120.0, -235.0],
            hi: [1195.0, 520.0, 215.0],
        },
        &ts,
        "room.jsonl",
    );
}

#[test]
#[ignore = "DIAGNOSTIC (task 5b): dumps em_sweeper's probes for the face artifacts"]
fn diag_sweeper_probe_dump() {
    let ts: Vec<f32> = vec![
        0.0, 0.05, 0.10, 0.143, 0.15, 0.20, 0.25, 0.286, 0.30, 0.70, 0.75, 0.80, 0.85, 1.0,
    ];
    dump_region(
        "enemy-movers",
        "em_sweeper",
        Region {
            lo: [1230.0, 30.0, -45.0],
            hi: [1640.0, 480.0, 140.0],
        },
        &ts,
        "sweeper.jsonl",
    );
}

/// Later-share errors at AC 19's probe groups (question 4): (i) the
/// termini's blended shares as `mid_pose::blended_share` forms them, (ii)
/// id 34's own later share, each against (iii) the reference's true later
/// share at t; plus the true share's own size. Display levels on the
/// reference's bounce one plus each share against the reference's full
/// state.
fn later_shares(map: &str, mover: &str, face: Option<Face>, ts: &[f32]) {
    let text = std::fs::read_to_string(fixture_path(map)).expect("dev map");
    let dir = tempfile::tempdir().expect("maps dir");
    let write = |name: &str, text: &str| {
        let path = dir.path().join(format!("{name}.map"));
        std::fs::write(&path, text).expect("write map");
        path
    };
    let original = write(map, &text);
    let build = Build::bake(&original, mover);
    let base_fill0 = base_at_fill(&original, 0.0);
    let analysed_all = build.scene.mover_poses();
    let analysed = analysed_all
        .analysed
        .iter()
        .find(|analysed| analysed.mover_id == build.mover_id)
        .expect("the mover");
    let posed = build.scene.poses();
    let solids = &posed
        .movers()
        .iter()
        .find(|posed| posed.mover_id == build.mover_id)
        .expect("posed")
        .solids;
    let first = &analysed.poses[0].bounds;
    let last = &analysed.poses[analysed.poses.len() - 1].bounds;
    let at = |t: f32, fill: f32| {
        let offset: DVec3 = (last.min - first.min) * f64::from(t);
        let path = write(
            &format!("{map}-{mover}-at-{t}-fill-{fill}"),
            &staticize_at(&text, mover, quake_offset(offset)),
        );
        base_at_fill(&path, fill)
    };
    // Every pose's posed bounce one over the domain, as the bake gathers it
    // (the full re-bake hands it back), keyed by (pose, probe).
    let pose_count = analysed.poses.len();
    let one_by_pose: HashMap<(usize, u32), [f32; 27]> = {
        let scene = Scene::of(load_map(&original));
        with_ctx(&scene.fixture, |ctx, poses| {
            let mut plan = plan_half(&scene.fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            let mut request = std::mem::take(&mut plan.request);
            request.jobs.retain(|job| job.mover_id == build.mover_id);
            request.keep_poses = true;
            let (_, _, outcome) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
            outcome
                .poses
                .into_iter()
                .filter(|baked| baked.mover_id == build.mover_id)
                .flat_map(|baked| {
                    let pose = baked.pose;
                    baked
                        .domain
                        .into_iter()
                        .map(move |probe| ((pose, probe.probe), probe.coefficients))
                })
                .collect()
        })
    };
    let one_tile = |pose: usize, p: usize| -> Option<Vec<[f32; 3]>> {
        one_by_pose
            .get(&(pose, p as u32))
            .map(|coefficients| {
                crate::sh_bake::octahedral_irradiance_tile_rgb(
                    coefficients,
                    DEFAULT_IRRADIANCE_TILE_DIMENSION,
                    DEFAULT_IRRADIANCE_TILE_BORDER,
                )
            })
    };
    let spacing = f64::from(CONFIG.probe_spacing) * QUAKE_PER_METRE;
    let decode = |tile: &[OctahedralAtlasTexel]| -> Vec<[f32; 3]> { tile.iter().map(super::rgb).collect() };
    let add = |a: &[[f32; 3]], b: &[[f32; 3]]| -> Vec<OctahedralAtlasTexel> {
        a.iter()
            .zip(b)
            .map(|(a, b)| super::texel([0, 1, 2].map(|c| a[c] + b[c])))
            .collect()
    };
    let sub = |a: &[[f32; 3]], b: &[[f32; 3]]| -> Vec<[f32; 3]> {
        a.iter().zip(b).map(|(a, b)| [0, 1, 2].map(|c| a[c] - b[c])).collect()
    };
    let mut out = out_file(&format!("later-shares-{map}-{}.txt", if face.is_none() { "inside" } else { "face" }));
    for &t in ts {
        let full = at(t, DEFAULT_BOUNCE_FILL);
        let one = at(t, 0.0);
        let offset: DVec3 = (last.min - first.min) * f64::from(t);
        let corners = [quake_offset(first.min + offset), quake_offset(first.max + offset)];
        let lo: [f64; 3] = std::array::from_fn(|a| corners[0][a].min(corners[1][a]));
        let hi: [f64; 3] = std::array::from_fn(|a| corners[0][a].max(corners[1][a]));
        let within = |q: [f64; 3], a: usize| lo[a] <= q[a] && q[a] <= hi[a];
        let layout = &build.scene.layout;
        let group: Vec<usize> = (0..layout.probe_positions.len())
            .filter(|&p| {
                layout.validity[p] != 0
                    && bounce_at(&full, layout.probe_positions[p]).is_some()
                    && bounce_at(&one, layout.probe_positions[p]).is_some()
            })
            .filter(|&p| {
                let q = quake_offset(layout.probe_positions[p]);
                match face {
                    Some(Face::West) => lo[0] - spacing < q[0] && q[0] < lo[0] && within(q, 1) && within(q, 2),
                    Some(Face::Top) => hi[2] < q[2] && q[2] < hi[2] + spacing && within(q, 0) && within(q, 1),
                    // Inside the rising room: within its inner walls, above
                    // its floor slab and below its roof slab (16 u each).
                    None => {
                        lo[0] + 16.0 < q[0]
                            && q[0] < hi[0] - 16.0
                            && lo[1] + 16.0 < q[1]
                            && q[1] < hi[1] - 16.0
                            && lo[2] + 16.0 < q[2]
                            && q[2] < hi[2] - 16.0
                    }
                }
            })
            .collect();
        let mut rows: Vec<[f32; 6]> = Vec::new();
        for &p in &group {
            let position = layout.probe_positions[p];
            let truth_full = bounce_at(&full, position).expect("valid");
            let truth_one = decode(&bounce_at(&one, position).expect("valid"));
            let _truth_share = sub(&decode(&truth_full), &truth_one);
            // (ii) id 34's later share, mover absent.
            let id34_share = sub(
                &decode(&build.base_tile(p)),
                &decode(&bounce_at(&base_fill0, position).expect("valid")),
            );
            // (i) the termini's shares, blended as `blended_share`.
            let point = vec3_from(layout.probe_positions[p]);
            let terminus = |k: usize| -> Option<Vec<[f32; 3]>> {
                let pose = if k == 0 { 0 } else { pose_count - 1 };
                if solids[pose].encloses(point) {
                    return None;
                }
                let state = build.bounce(p, if k == 0 { 0.0 } else { 1.0 });
                let one = one_tile(pose, p)?;
                Some(sub(&decode(&state), &one))
            };
            let blended = match (terminus(0), terminus(1)) {
                (Some(a), Some(b)) => Some(
                    a.iter()
                        .zip(&b)
                        .map(|(a, b)| [0, 1, 2].map(|c| (1.0 - t) * a[c] + t * b[c]))
                        .collect::<Vec<_>>(),
                ),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            };
            // (v) the blended shares scaled, texel by texel, by the pose's
            // bounce one over the termini's bounce one blended alike (ratio
            // capped at 4): later light follows first light locally.
            let terminus_one = |k: usize| -> Option<Vec<[f32; 3]>> {
                let pose = if k == 0 { 0 } else { pose_count - 1 };
                (!solids[pose].encloses(point)).then(|| one_tile(pose, p)).flatten()
            };
            let blended_one = match (terminus_one(0), terminus_one(1)) {
                (Some(a), Some(b)) => Some(
                    a.iter()
                        .zip(&b)
                        .map(|(a, b)| [0, 1, 2].map(|c| (1.0 - t) * a[c] + t * b[c]))
                        .collect::<Vec<_>>(),
                ),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            };
            let err_scaled = match (&blended, &blended_one) {
                (Some(share), Some(one_t)) => {
                    let scaled: Vec<[f32; 3]> = share
                        .iter()
                        .zip(one_t)
                        .zip(&truth_one)
                        .map(|((later, b1t), b1)| {
                            [0, 1, 2].map(|c| {
                                let ratio = if b1t[c] > 1.0e-4 { (b1[c] / b1t[c]).min(4.0) } else { 1.0 };
                                later[c] * ratio
                            })
                        })
                        .collect();
                    display_levels(&add(&truth_one, &scaled), &truth_full)
                }
                _ => f32::NAN,
            };
            let err_blended = blended
                .as_ref()
                .map_or(f32::NAN, |share| display_levels(&add(&truth_one, share), &truth_full));
            let err_id34 = display_levels(&add(&truth_one, &id34_share), &truth_full);
            // (iv) a local scaling: the pose's bounce one times id 34's own
            // later-to-first ratio at the probe, texel by texel.
            let base_one = decode(&bounce_at(&base_fill0, position).expect("valid"));
            let ratio_share: Vec<[f32; 3]> = truth_one
                .iter()
                .zip(&id34_share)
                .zip(&base_one)
                .map(|((one, later), b1)| {
                    [0, 1, 2].map(|c| if b1[c] > 1.0e-6 { one[c] * later[c] / b1[c] } else { 0.0 })
                })
                .collect();
            let err_ratio = display_levels(&add(&truth_one, &ratio_share), &truth_full);
            let size = display_levels(&bounce_at(&one, position).expect("valid"), &truth_full);
            let composed = display_levels(&build.bounce(p, t), &truth_full);
            rows.push([err_blended, err_id34, size, composed, err_ratio, err_scaled]);
            writeln!(
                out,
                "{map} t {t:.2} probe {p} quake {:?}: blended {err_blended:.2}, id34 {err_id34:.2}, ratio {err_ratio:.2}, true-share size {size:.2}, composed {composed:.2}",
                quake_offset(layout.probe_positions[p]).map(|v| v.round())
            )
            .unwrap();
        }
        let stat = |i: usize| {
            let values: Vec<f32> = rows.iter().map(|r| r[i]).filter(|v| v.is_finite()).collect();
            let worst = values.iter().copied().fold(0.0, f32::max);
            let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
            (worst, mean, values.len())
        };
        let summary = format!(
            "SUMMARY {map} {mover} t {t:.2}: {} probes; blended termini shares worst/mean {:.2}/{:.2} (n {}); id34 share {:.2}/{:.2}; true later share size {:.2}/{:.2}; composed bounce (AC 19) {:.2}/{:.2}; bounce one x id34 ratio {:.2}/{:.2}; termini shares scaled by bounce-one ratio {:.2}/{:.2}",
            group.len(),
            stat(0).0,
            stat(0).1,
            stat(0).2,
            stat(1).0,
            stat(1).1,
            stat(2).0,
            stat(2).1,
            stat(3).0,
            stat(3).1,
            stat(4).0,
            stat(4).1,
            stat(5).0,
            stat(5).1,
        );
        eprintln!("{summary}");
        writeln!(out, "{summary}").unwrap();
    }
}

#[test]
#[ignore = "DIAGNOSTIC (task 5b): later-share errors at AC 19's groups"]
fn diag_later_shares() {
    later_shares("enemy-movers", "em_sweeper", Some(Face::West), &[0.25, 0.30]);
    later_shares("mover-setpiece", "setpiece_room", Some(Face::Top), &[0.4, 0.6]);
}

#[test]
#[ignore = "DIAGNOSTIC (task 5b): later-share errors inside the rising room"]
fn diag_later_shares_inside_the_room() {
    later_shares("mover-setpiece", "setpiece_room", None, &[0.3, 0.4, 0.5, 0.6]);
}
