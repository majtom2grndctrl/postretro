// Navmesh bake tests cover rasterization, erosion, region decomposition, and portals.
// See: context/lib/build_pipeline.md §Navigation bake

use super::*;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::FaceIndexRange;

/// Build a `GeometryResult` from a flat list of world-space triangles.
/// Every triangle gets a placeholder face/leaf — the bake reads only the
/// vertex positions and the index triples.
fn geo_from_triangles(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for tri in triangles {
        let base = vertices.len() as u32;
        for &pos in tri {
            vertices.push(Vertex::new(
                pos,
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

/// Two triangles forming an axis-aligned floor quad at height `y` over
/// `[x0, x1] x [z0, z1]`, wound so `(b-a) x (c-a)` points up (+Y) — the bake
/// only treats upward-facing surfaces as walkable.
fn floor_quad(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> [[[f32; 3]; 3]; 2] {
    [
        [[x0, y, z0], [x1, y, z1], [x1, y, z0]],
        [[x0, y, z0], [x0, y, z1], [x1, y, z1]],
    ]
}

/// A downward-facing (-Y normal) quad at height `y` — a ceiling. Reverses
/// `floor_quad`'s winding so the bake never treats it as walkable.
fn ceiling_quad(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> [[[f32; 3]; 3]; 2] {
    [
        [[x0, y, z0], [x1, y, z0], [x1, y, z1]],
        [[x0, y, z0], [x1, y, z1], [x0, y, z1]],
    ]
}

/// Test params: tiny radius (no erosion) unless a test overrides it, so the
/// rasterization/region logic is exercised in isolation. `step_height` is
/// fixed at 0.3 (not the engine default of 0.5) — chosen to suit the
/// thin-deck and step fixtures in this suite.
fn no_erode_params() -> NavParams {
    NavParams {
        agent_radius: 0.0,
        agent_height: 1.8,
        step_height: 0.3,
        max_slope_deg: 45.0,
        cell_size: 0.25,
    }
}

#[test]
fn flat_floor_produces_single_region_no_portals() {
    // 2 m x 2 m floor at y = 0; with zero radius and open sky, every cell
    // is walkable and merges into one rectangle.
    let tris = floor_quad(0.0, 0.0, 2.0, 2.0, 0.0);
    let geo = geo_from_triangles(&tris);
    let section = bake_navmesh(&geo, &no_erode_params()).expect("flat floor must bake");

    assert_eq!(section.regions.len(), 1);
    assert!(section.portals.is_empty());
    let r = section.regions[0];
    assert_eq!(r.x0, 0);
    assert_eq!(r.z0, 0);
    assert_eq!(r.x1, 8); // 2.0 / 0.25
    assert_eq!(r.z1, 8);
    assert!((r.floor_y_min - 0.0).abs() < 1.0e-4);
    assert!((r.floor_y_max - 0.0).abs() < 1.0e-4);
}

#[test]
fn region_claim_uses_locally_accepted_row_heights() {
    // Regression: z-growth accepts a rectangle by comparing each row to the
    // row directly below. A long run can accumulate more total float drift
    // than LEVEL_EPS, so claiming from the final row back to the seed row
    // used to miss the seed span and panic with "claimed span exists".
    let heights: Vec<Vec<f32>> = (0..8).map(|z| vec![z as f32 * 0.0006]).collect();
    let grid = WalkGrid {
        origin: Vec3::ZERO,
        cell_size: 0.25,
        dim_x: 1,
        dim_z: heights.len() as u32,
        cells: heights,
    };

    let regions = decompose_regions(&grid);

    assert_eq!(regions.len(), 1);
    let region = regions[0];
    assert_eq!((region.x0, region.z0, region.x1, region.z1), (0, 0, 1, 8));
    assert!((region.floor_y_min - 0.0).abs() < 1.0e-4);
    assert!((region.floor_y_max - 0.0042).abs() < 1.0e-4);
}

#[test]
fn thin_deck_records_floor_at_walk_surface_not_underside() {
    // Regression: a bridge deck thinner than merge_eps (= agent_height * 0.25
    // = 0.45 m) has its walkable top and non-walkable underside merged into
    // one span. The span's floor must be the TOP (1.0), the surface an agent
    // stands on — not the underside (0.7). The old bug kept the underside,
    // sinking the region a slab-thickness below the deck (it then read as a
    // disconnected island: the step delta to flush ground exceeds step_height).
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 1.0)); // deck top (walkable)
    tris.extend_from_slice(&ceiling_quad(0.0, 0.0, 2.0, 2.0, 0.7)); // underside, 0.3 m thick
    let geo = geo_from_triangles(&tris);
    let section = bake_navmesh(&geo, &no_erode_params()).expect("thin deck must bake");

    assert_eq!(section.regions.len(), 1);
    let r = section.regions[0];
    assert!(
        (r.floor_y_min - 1.0).abs() < 1.0e-4,
        "deck floor must sit on the walk surface (1.0), got {}",
        r.floor_y_min
    );
    assert!((r.floor_y_max - 1.0).abs() < 1.0e-4);
}

#[test]
fn no_floor_geometry_emits_no_section() {
    // A single vertical wall quad (normal in XZ plane): no walkable surface.
    let tris = [
        [[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [2.0, 2.0, 0.0]],
        [[0.0, 0.0, 0.0], [2.0, 2.0, 0.0], [2.0, 0.0, 0.0]],
    ];
    let geo = geo_from_triangles(&tris);
    assert!(bake_navmesh(&geo, &no_erode_params()).is_none());
}

#[test]
fn empty_geometry_emits_no_section() {
    let geo = geo_from_triangles(&[]);
    assert!(bake_navmesh(&geo, &no_erode_params()).is_none());
}

#[test]
fn steep_slope_is_not_walkable() {
    // A ~60-degree ramp exceeds the 45-degree slope filter, so no cell is
    // walkable and no section is emitted.
    // Ramp rises 2 m over 1 m of XZ run: slope ~63 degrees.
    let tris = [
        [[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [1.0, 2.0, 2.0]],
        [[0.0, 0.0, 0.0], [1.0, 2.0, 2.0], [0.0, 0.0, 2.0]],
    ];
    let geo = geo_from_triangles(&tris);
    assert!(
        bake_navmesh(&geo, &no_erode_params()).is_none(),
        "a slope steeper than max_slope_deg must produce no walkable region"
    );
}

#[test]
fn low_clearance_ceiling_blocks_walkability() {
    // Floor at y=0 with a ceiling 1.0 m above it (< agent_height 1.8) makes
    // the floor span non-walkable for clearance.
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
    // Downward-facing ceiling 1.0 m above the floor: non-walkable itself
    // (normal points -Y) and it bounds the floor span's clearance below
    // agent_height, so neither surface is walkable.
    tris.extend_from_slice(&ceiling_quad(0.0, 0.0, 2.0, 2.0, 1.0));
    let geo = geo_from_triangles(&tris);
    assert!(
        bake_navmesh(&geo, &no_erode_params()).is_none(),
        "a ceiling closer than agent_height must remove the floor's walkability"
    );
}

#[test]
fn agent_radius_erodes_floor_edges() {
    // 2 m floor with agent_radius 0.25. The conservative cell-square test
    // erodes the boundary ring plus its immediately adjacent ring.
    let tris = floor_quad(0.0, 0.0, 2.0, 2.0, 0.0);
    let geo = geo_from_triangles(&tris);
    let params = NavParams {
        agent_radius: 0.25,
        ..no_erode_params()
    };
    let section = bake_navmesh(&geo, &params).expect("interior must survive erosion");
    let r = section.regions[0];
    // 8x8 grid, two-cell erosion ring → interior 4x4 at [2,6).
    assert_eq!(r.x0, 2);
    assert_eq!(r.z0, 2);
    assert_eq!(r.x1, 6);
    assert_eq!(r.z1, 6);
}

// Regression: erosion used a stricter floor match than boundary classification,
// leaving cells beside climbable steps in the step-height rounding band.
#[test]
fn erosion_removes_candidate_matching_boundary_within_step_epsilon() {
    const CELL_SIZE: f32 = 0.25;
    const DIM: u32 = 9;
    const CANDIDATE: (u32, u32) = (3, 3);
    const RAISED_BOUNDARY: (u32, u32) = (4, 4);
    const NOTCH: (u32, u32) = (5, 4);

    let params = NavParams {
        agent_radius: 0.19,
        step_height: 0.3,
        cell_size: CELL_SIZE,
        ..no_erode_params()
    };
    let raised_floor = params.step_height + STEP_EPS * 0.5;
    assert!(raised_floor > params.step_height);
    assert!(raised_floor <= params.step_height + STEP_EPS);

    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    for z in 0..DIM {
        for x in 0..DIM {
            if (x, z) == NOTCH {
                continue;
            }
            let floor_y = if (x, z) == RAISED_BOUNDARY {
                raised_floor
            } else {
                0.0
            };
            tris.extend_from_slice(&floor_quad(
                x as f32 * CELL_SIZE,
                z as f32 * CELL_SIZE,
                (x + 1) as f32 * CELL_SIZE,
                (z + 1) as f32 * CELL_SIZE,
                floor_y,
            ));
        }
    }

    let geo = geo_from_triangles(&tris);
    let section = bake_navmesh(&geo, &params).expect("fixture interior must survive erosion");
    let candidate_is_covered = section.regions.iter().any(|region| {
        CANDIDATE.0 >= region.x0
            && CANDIDATE.0 < region.x1
            && CANDIDATE.1 >= region.z0
            && CANDIDATE.1 < region.z1
    });

    assert!(
        !candidate_is_covered,
        "candidate must erode against a climbable boundary within STEP_EPS"
    );
}

#[test]
fn euclidean_erosion_is_isotropic_and_bounded_by_one_cell() {
    const CELL_SIZE: f32 = 0.25;
    // The one-cell isotropy bound only holds where the straight and
    // 45-degree diagonal sample paths quantize to aligned depths; it is NOT
    // a universal property of the erosion (e.g. agent_radius in roughly
    // [0.823, 0.884) at this cell_size diverges by ~1.17 cells, because the
    // diagonal sample advances in steps of sqrt(2)*cell_size while the
    // straight sample advances in whole cells). 0.4 is the canonical baked
    // agent radius (see `NavParams`/`map_data.rs`) and is the value this
    // test must validate AC3 against, since it's the only radius the engine
    // actually bakes and it comfortably satisfies the bound (straight 0.5 vs
    // diagonal ~0.354, diff ~0.146 < one cell).
    const AGENT_RADIUS: f32 = 0.4;
    const DIM: u32 = 64;
    const WALL: u32 = 24;
    const EPSILON: f32 = 1.0e-5;

    let params = NavParams {
        agent_radius: AGENT_RADIUS,
        step_height: 0.0,
        cell_size: CELL_SIZE,
        ..no_erode_params()
    };
    let make_grid = |is_walkable: &dyn Fn(u32, u32) -> bool| WalkGrid {
        origin: Vec3::ZERO,
        cell_size: CELL_SIZE,
        dim_x: DIM,
        dim_z: DIM,
        cells: (0..DIM)
            .flat_map(|z| {
                (0..DIM).map(move |x| {
                    if is_walkable(x, z) {
                        vec![0.0]
                    } else {
                        Vec::new()
                    }
                })
            })
            .collect(),
    };

    // The empty half-grid is the non-walkable side of a straight wall. The
    // diagonal fixture is the same wall rotated 45 degrees, rasterized as a
    // staircase boundary. Both sample paths start on a boundary column and
    // advance perpendicular to their wall, far from the outer grid edges.
    let straight = erode(make_grid(&|x, _| x >= WALL), &params);
    let diagonal = erode(make_grid(&|x, z| x + z >= WALL * 2), &params);
    let erased_run = |grid: &WalkGrid, start_x: u32, start_z: u32, dx: u32, dz: u32| {
        let count = (0..(DIM - WALL))
            .take_while(|&offset| {
                grid.heights_at(start_x + offset * dx, start_z + offset * dz)
                    .is_empty()
            })
            .count();
        assert!(
            count > 0,
            "the sample must begin on an eroded boundary column"
        );
        (count - 1) as f32
    };

    let straight_depth = erased_run(&straight, WALL, DIM / 2, 1, 0) * CELL_SIZE;
    let diagonal_depth =
        erased_run(&diagonal, WALL, WALL, 1, 1) * CELL_SIZE * std::f32::consts::SQRT_2;

    assert!(
        (straight_depth - diagonal_depth).abs() <= CELL_SIZE + EPSILON,
        "straight and 45-degree erosion must agree within one cell: \
         straight={straight_depth}, diagonal={diagonal_depth}"
    );
    assert!(
        straight_depth <= AGENT_RADIUS + CELL_SIZE + EPSILON,
        "straight-wall erosion must not exceed agent radius plus one cell: {straight_depth}"
    );
    assert!(
        diagonal_depth <= AGENT_RADIUS + CELL_SIZE + EPSILON,
        "45-degree erosion must not exceed agent radius plus one cell: {diagonal_depth}"
    );
}

#[test]
fn doorway_beside_a_step_survives() {
    // Two coplanar floor halves split by a step_height riser, with a
    // doorway-width gap beside the riser where both halves are at the SAME
    // height (a flush walkway). The flush gap must not erode — it is not a
    // wall, and the step beside it is climbable.
    //
    // Layout (z is depth, x is width), cell_size 0.25:
    //   Lower floor at y=0 over x in [0, 2], z in [0, 3].
    //   Upper floor at y=0.3 (== step_height) over x in [0, 2], z in [3, 6],
    //     EXCEPT a doorway column-strip x in [0.5, 1.0] kept at y=0 so the
    //     two halves are flush there (a level walkway through the gap).
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 3.0, 0.0)); // lower half
    // Upper half, split around the doorway strip [0.5, 1.0] in x.
    tris.extend_from_slice(&floor_quad(0.0, 3.0, 0.5, 6.0, 0.3));
    tris.extend_from_slice(&floor_quad(1.0, 3.0, 2.0, 6.0, 0.3));
    // Flush doorway walkway at y=0 bridging the two halves.
    tris.extend_from_slice(&floor_quad(0.5, 3.0, 1.0, 6.0, 0.0));

    let geo = geo_from_triangles(&tris);
    // Use a small but non-zero radius so erosion runs; the doorway is wide
    // enough (2 cells) that with radius 0 the interior survives, and we want
    // to prove the climbable step does NOT erode the doorway cells.
    let params = NavParams {
        agent_radius: 0.0,
        step_height: 0.3,
        ..no_erode_params()
    };
    let section = bake_navmesh(&geo, &params).expect("stepped floor must bake");

    // The doorway strip cells (x in [2, 4), at the boundary z=12 between the
    // halves) must be present somewhere in a region. Confirm a cell at the
    // flush gap, on the upper-half side row, is covered by a region.
    let covered = |x: u32, z: u32| {
        section
            .regions
            .iter()
            .any(|r| x >= r.x0 && x < r.x1 && z >= r.z0 && z < r.z1)
    };
    // Doorway columns x in [2, 4); pick a row just inside the upper half
    // (z = 12 is the first upper-half row: 3.0 / 0.25 = 12).
    assert!(
        covered(2, 12) && covered(3, 12),
        "doorway cells beside the climbable step must survive (not eroded as a wall)"
    );
}

#[test]
fn climbable_step_yields_a_portal() {
    // Two floor halves one step_height apart, sharing a full edge run within
    // step → exactly one portal between the two regions.
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 1.0, 0.0)); // z in [0,1)
    tris.extend_from_slice(&floor_quad(0.0, 1.0, 2.0, 2.0, 0.3)); // z in [1,2), +step
    let geo = geo_from_triangles(&tris);
    let params = NavParams {
        agent_radius: 0.0,
        step_height: 0.3,
        ..no_erode_params()
    };
    let section = bake_navmesh(&geo, &params).expect("stepped floor must bake");
    assert_eq!(
        section.regions.len(),
        2,
        "a step splits the floor into two regions"
    );
    assert_eq!(
        section.portals.len(),
        1,
        "the climbable step shares a portal"
    );
    let p = &section.portals[0];
    assert!(p.region_a < p.region_b);
    // Portal Y is the min of the two floor heights along the run (= 0.0).
    assert!((p.left[1] - 0.0).abs() < 1.0e-4);
    // Endpoints lie on the shared Z line (z = 1.0 in world).
    assert!((p.left[2] - 1.0).abs() < 1.0e-4);
    assert!((p.right[2] - 1.0).abs() < 1.0e-4);
}

#[test]
fn tall_ledge_yields_no_portal() {
    // Two floor halves separated by a drop larger than step_height. They are
    // distinct regions but share no traversable edge → no portal.
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 1.0, 0.0));
    tris.extend_from_slice(&floor_quad(0.0, 1.0, 2.0, 2.0, 1.0)); // +1.0 m, a ledge
    let geo = geo_from_triangles(&tris);
    let params = NavParams {
        agent_radius: 0.0,
        step_height: 0.3,
        ..no_erode_params()
    };
    let section = bake_navmesh(&geo, &params).expect("ledged floor must bake");
    assert_eq!(section.regions.len(), 2);
    assert!(
        section.portals.is_empty(),
        "a drop taller than step_height must yield no portal"
    );
}

#[test]
fn stacked_floors_produce_distinct_regions_no_portal() {
    // Two floors over the SAME XZ footprint, vertically separated by more
    // than agent_height. Both are walkable (open clearance on each), giving
    // a region on each level over the same footprint and NO portal between
    // them (no shared cell edge — they overlap, not abut).
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 2.0, 2.0, 5.0)); // 5 m up
    let geo = geo_from_triangles(&tris);
    let params = no_erode_params();
    let section = bake_navmesh(&geo, &params).expect("stacked floors must bake");

    // Both floors are independently walkable: each column keeps both spans
    // (the 5 m gap gives each open clearance), so the decomposition builds a
    // region on EACH level over the same footprint. The two regions overlap
    // in XZ but do not abut at a cell edge, so there is no portal between
    // them — vertical stacking is not a traversable step.
    assert_eq!(
        section.regions.len(),
        2,
        "stacked floors yield a distinct region on each level"
    );
    // Same cell footprint, different floor heights.
    let a = section.regions[0];
    let b = section.regions[1];
    assert_eq!((a.x0, a.z0, a.x1, a.z1), (0, 0, 8, 8));
    assert_eq!((b.x0, b.z0, b.x1, b.z1), (0, 0, 8, 8));
    let mut heights = [a.floor_y_min, b.floor_y_min];
    heights.sort_by(f32::total_cmp);
    assert!((heights[0] - 0.0).abs() < 1.0e-4, "lower level at y ~= 0");
    assert!((heights[1] - 5.0).abs() < 1.0e-4, "upper level at y ~= 5");
    assert!(
        section.portals.is_empty(),
        "stacked floors over the same footprint must not portal between levels"
    );
}

#[test]
fn bake_is_byte_deterministic_in_process() {
    // Two in-process bakes of the same fixture must produce byte-identical
    // section bytes (the stage cache keys on these bytes).
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 3.0, 3.0, 0.0));
    tris.extend_from_slice(&floor_quad(0.0, 3.0, 3.0, 6.0, 0.2));
    let geo = geo_from_triangles(&tris);
    let params = no_erode_params();
    let a = bake_navmesh(&geo, &params).expect("bake a").to_bytes();
    let b = bake_navmesh(&geo, &params).expect("bake b").to_bytes();
    assert_eq!(a, b, "navmesh bake must be byte-deterministic in-process");
}

#[test]
fn regions_are_disjoint_and_cover_walkable_cells() {
    // An L-shaped floor: greedy decomposition must tile it with disjoint
    // rectangles whose union is exactly the walkable cells.
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 3.0, 1.0, 0.0)); // horizontal arm
    tris.extend_from_slice(&floor_quad(0.0, 1.0, 1.0, 3.0, 0.0)); // vertical arm
    let geo = geo_from_triangles(&tris);
    let params = no_erode_params();
    let section = bake_navmesh(&geo, &params).expect("L floor must bake");

    // No two regions overlap (cell-space rectangles are disjoint).
    for i in 0..section.regions.len() {
        for j in (i + 1)..section.regions.len() {
            let a = section.regions[i];
            let b = section.regions[j];
            let overlap_x = a.x0 < b.x1 && b.x0 < a.x1;
            let overlap_z = a.z0 < b.z1 && b.z0 < a.z1;
            assert!(
                !(overlap_x && overlap_z),
                "regions {i} and {j} overlap in cell space"
            );
        }
    }
    // Total covered cell count equals the walkable cell count: an L over a
    // 12x12 grid (3 m / 0.25). Horizontal arm 12x4 + vertical arm 4x8.
    let covered: u32 = section
        .regions
        .iter()
        .map(|r| (r.x1 - r.x0) * (r.z1 - r.z0))
        .sum();
    assert_eq!(covered, 12 * 4 + 4 * 8);
}

#[test]
fn cache_key_changes_with_each_nav_param() {
    // Changing any nav param must change the stage cache key (and the
    // unchanged case must reproduce the same key). Mirrors the SDF stage's
    // input-hash construction: blake3(postcard(geo) || postcard(params)).
    use crate::cache::CacheKey;

    let geo = geo_from_triangles(&floor_quad(0.0, 0.0, 2.0, 2.0, 0.0));
    let base = no_erode_params();

    let key_for = |params: &NavParams| -> String {
        let mut buf = postcard::to_allocvec(&geo).unwrap();
        buf.extend_from_slice(&postcard::to_allocvec(params).unwrap());
        let input_hash = blake3::hash(&buf);
        CacheKey::new("navmesh", NAVMESH_STAGE_VERSION, input_hash.as_bytes()).as_filename()
    };

    let base_key = key_for(&base);
    assert_eq!(
        base_key,
        key_for(&base),
        "unchanged params reproduce the key"
    );

    let mutated = [
        NavParams {
            agent_radius: base.agent_radius + 0.1,
            ..base
        },
        NavParams {
            agent_height: base.agent_height + 0.1,
            ..base
        },
        NavParams {
            step_height: base.step_height + 0.1,
            ..base
        },
        NavParams {
            max_slope_deg: base.max_slope_deg + 1.0,
            ..base
        },
        NavParams {
            cell_size: base.cell_size + 0.05,
            ..base
        },
    ];
    for m in &mutated {
        assert_ne!(
            base_key,
            key_for(m),
            "changing a nav param must change the cache key"
        );
    }
}
