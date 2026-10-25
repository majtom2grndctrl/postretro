// Synthetic portal worlds for region-walk tests: rectilinear cell grids with
// real portals and a real locator, plus camera sampling.

use glam::{Mat4, Vec3};

use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData,
};

/// A rectilinear grid world: cell `(i, j, k)` spans `xs[i]..xs[i+1]`,
/// `ys[j]..ys[j+1]`, `zs[k]..zs[k+1]`. Every face shared by two empty cells
/// is a portal; solid cells keep their place in the locator. Portal winding
/// and front/back assignment alternate so no test leans on a convention.
pub(crate) struct GridWorld {
    pub world: LevelWorld,
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    pub zs: Vec<f32>,
}

impl GridWorld {
    pub(crate) fn new(
        xs: Vec<f32>,
        ys: Vec<f32>,
        zs: Vec<f32>,
        solid: impl Fn(usize, usize, usize) -> bool,
    ) -> Self {
        let (nx, ny, nz) = (xs.len() - 1, ys.len() - 1, zs.len() - 1);
        let index = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
        let mut cells = Vec::with_capacity(nx * ny * nz);
        let mut face_start = 0;
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let is_solid = solid(i, j, k);
                    cells.push(CellData {
                        bounds_min: Vec3::new(xs[i], ys[j], zs[k]),
                        bounds_max: Vec3::new(xs[i + 1], ys[j + 1], zs[k + 1]),
                        face_start,
                        face_count: u32::from(!is_solid),
                        portal_ref_start: 0,
                        portal_ref_count: 0,
                        is_solid,
                        is_exterior: false,
                        is_drawable: !is_solid,
                    });
                    face_start += u32::from(!is_solid);
                }
            }
        }

        let mut portals = Vec::new();
        let mut refs_by_cell: Vec<Vec<u32>> = vec![Vec::new(); cells.len()];
        let mut add = |a: usize, b: usize, polygon: Vec<Vec3>| {
            if cells[a].is_solid || cells[b].is_solid {
                return;
            }
            let id = portals.len() as u32;
            let (front, back, polygon) = if id.is_multiple_of(2) {
                (a, b, polygon)
            } else {
                (b, a, polygon.into_iter().rev().collect())
            };
            portals.push(PortalData {
                polygon,
                front_cell: front,
                back_cell: back,
            });
            refs_by_cell[a].push(id);
            refs_by_cell[b].push(id);
        };
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let here = index(i, j, k);
                    if i + 1 < nx {
                        let x = xs[i + 1];
                        add(
                            here,
                            index(i + 1, j, k),
                            quad(
                                Vec3::new(x, ys[j], zs[k]),
                                Vec3::new(x, ys[j + 1], zs[k]),
                                Vec3::new(x, ys[j + 1], zs[k + 1]),
                                Vec3::new(x, ys[j], zs[k + 1]),
                            ),
                        );
                    }
                    if j + 1 < ny {
                        let y = ys[j + 1];
                        add(
                            here,
                            index(i, j + 1, k),
                            quad(
                                Vec3::new(xs[i], y, zs[k]),
                                Vec3::new(xs[i + 1], y, zs[k]),
                                Vec3::new(xs[i + 1], y, zs[k + 1]),
                                Vec3::new(xs[i], y, zs[k + 1]),
                            ),
                        );
                    }
                    if k + 1 < nz {
                        let z = zs[k + 1];
                        add(
                            here,
                            index(i, j, k + 1),
                            quad(
                                Vec3::new(xs[i], ys[j], z),
                                Vec3::new(xs[i + 1], ys[j], z),
                                Vec3::new(xs[i + 1], ys[j + 1], z),
                                Vec3::new(xs[i], ys[j + 1], z),
                            ),
                        );
                    }
                }
            }
        }

        let mut refs = Vec::new();
        for (cell, cell_refs) in cells.iter_mut().zip(&refs_by_cell) {
            cell.portal_ref_start = refs.len() as u32;
            cell.portal_ref_count = cell_refs.len() as u32;
            refs.extend_from_slice(cell_refs);
        }

        let mut nodes = Vec::new();
        let root = locator(&xs, &ys, &zs, [0, 0, 0], [nx, ny, nz], &index, &mut nodes);
        let has_portals = !portals.is_empty();
        let world = LevelWorld::new_visibility_only(cells, refs, root, nodes, portals, has_portals)
            .expect("valid grid world");
        Self { world, xs, ys, zs }
    }

    pub(crate) fn cell_at(&self, i: usize, j: usize, k: usize) -> usize {
        let (nx, ny) = (self.xs.len() - 1, self.ys.len() - 1);
        i + nx * (j + ny * k)
    }

    pub(crate) fn centre(&self, cell: usize) -> Vec3 {
        let data = &self.world.cells[cell];
        (data.bounds_min + data.bounds_max) * 0.5
    }
}

fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Vec<Vec3> {
    vec![a, b, c, d]
}

/// Axis-split decision tree over the grid's index box `[lo, hi)`. On-plane
/// points go front, which is the higher-coordinate side, matching the cell
/// that owns the face's upper half-open range.
fn locator(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    lo: [usize; 3],
    hi: [usize; 3],
    index: &dyn Fn(usize, usize, usize) -> usize,
    nodes: &mut Vec<CellLocatorNodeData>,
) -> CellLocatorChild {
    let spans = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let axis = (0..3).max_by_key(|&a| spans[a]).expect("three axes");
    if spans[axis] == 1 {
        return CellLocatorChild::Cell(index(lo[0], lo[1], lo[2]));
    }
    let mid = lo[axis] + spans[axis] / 2;
    let coords = [xs, ys, zs][axis];
    let mut back_hi = hi;
    back_hi[axis] = mid;
    let mut front_lo = lo;
    front_lo[axis] = mid;
    let slot = nodes.len();
    nodes.push(CellLocatorNodeData {
        plane_normal: Vec3::ZERO,
        plane_distance: 0.0,
        front: CellLocatorChild::Cell(0),
        back: CellLocatorChild::Cell(0),
    });
    let front = locator(xs, ys, zs, front_lo, hi, index, nodes);
    let back = locator(xs, ys, zs, lo, back_hi, index, nodes);
    let mut normal = Vec3::ZERO;
    normal[axis] = 1.0;
    nodes[slot] = CellLocatorNodeData {
        plane_normal: normal,
        plane_distance: coords[mid],
        front,
        back,
    };
    CellLocatorChild::Node(slot)
}

/// A shaft of stacked thin slabs: `columns`² columns of `slabs` slabs each,
/// slab heights varying so boundaries sit at many depths. The stair-room
/// geometry that makes the chain walk explode (`research.md` §Why the exact
/// walk explodes).
pub(crate) fn stacked_slab_shaft(columns: usize, slabs: usize) -> GridWorld {
    let xs: Vec<f32> = (0..=columns).map(|i| i as f32 * 1.5).collect();
    let zs = xs.clone();
    let mut ys = vec![0.0];
    for s in 0..slabs {
        let height = 0.6 + 0.15 * ((s * 7) % 5) as f32;
        ys.push(ys[s] + height);
    }
    GridWorld::new(xs, ys, zs, |_, _, _| false)
}

/// An open `n`³ lattice of unit-ish cubes, no solids.
pub(crate) fn open_lattice(n: usize) -> GridWorld {
    let coords: Vec<f32> = (0..=n)
        .map(|i| i as f32 * 2.0 + (i % 2) as f32 * 0.3)
        .collect();
    GridWorld::new(coords.clone(), coords.clone(), coords, |_, _, _| false)
}

/// Stacked corridor rings around a solid core: every ring is a cycle, and
/// rings connect vertically.
pub(crate) fn cyclic_rings(side: usize, layers: usize) -> GridWorld {
    let xs: Vec<f32> = (0..=side).map(|i| i as f32 * 3.0).collect();
    let zs = xs.clone();
    let ys: Vec<f32> = (0..=layers).map(|j| j as f32 * 2.5).collect();
    GridWorld::new(xs, ys, zs, move |i, _, k| {
        i > 0 && i + 1 < side && k > 0 && k + 1 < side
    })
}

/// Deterministic generator for camera sweeps.
pub(crate) struct SplitMix64(pub u64);

impl SplitMix64 {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Runtime-default projection: 100° horizontal FOV at 16:9, near 0.1.
pub(crate) fn camera_view_proj(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    view_proj_with(eye, yaw, pitch, 100.0, 16.0 / 9.0)
}

pub(crate) fn view_proj_with(eye: Vec3, yaw: f32, pitch: f32, hfov_deg: f32, aspect: f32) -> Mat4 {
    let look = Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let up = if pitch.abs() > 1.5 { Vec3::Z } else { Vec3::Y };
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, up);
    let vfov = 2.0 * ((hfov_deg.to_radians() / 2.0).tan() / aspect).atan();
    glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0) * view
}

/// A camera pose sampled inside an empty cell, located by the fixture's own
/// locator.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CameraPose {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub cell: usize,
}

pub(crate) fn sample_cameras(grid: &GridWorld, count: usize, seed: u64) -> Vec<CameraPose> {
    let world = &grid.world;
    let empty: Vec<usize> = (0..world.cell_count())
        .filter(|&c| !world.cells[c].is_solid)
        .collect();
    let mut rng = SplitMix64(seed);
    let pitches = [
        0.0,
        45f32.to_radians(),
        -45f32.to_radians(),
        89f32.to_radians(),
        -89f32.to_radians(),
    ];
    (0..count)
        .map(|n| {
            let cell = empty[(rng.next_u64() % empty.len() as u64) as usize];
            let data = &world.cells[cell];
            let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.9 + Vec3::splat(0.05);
            let mut eye = data.bounds_min + (data.bounds_max - data.bounds_min) * fraction;
            // Every third camera sits on, or within the bypass distance of,
            // one of its cell's walls: the portal-plane cases.
            if n % 3 == 1 {
                let axis = (rng.next_u64() % 3) as usize;
                let wall = if rng.unit() < 0.5 {
                    data.bounds_min[axis]
                } else {
                    data.bounds_max[axis]
                };
                let offsets = [0.0, 0.004, -0.004, 0.02];
                eye[axis] = wall + offsets[(rng.next_u64() % 4) as usize];
            }
            CameraPose {
                eye,
                yaw: rng.unit() * std::f32::consts::TAU,
                pitch: pitches[n % pitches.len()],
                cell: world.locate_cell(eye),
            }
        })
        .collect()
}

/// A hand-built world of convex boxes and explicit portals. `locator` gives
/// the decision tree; [`axis_node`] builds its nodes.
pub(crate) fn boxes_world(
    boxes: &[(Vec3, Vec3)],
    portals: Vec<PortalData>,
    locator: (CellLocatorChild, Vec<CellLocatorNodeData>),
) -> LevelWorld {
    let mut cells: Vec<CellData> = boxes
        .iter()
        .enumerate()
        .map(|(i, &(bounds_min, bounds_max))| CellData {
            bounds_min,
            bounds_max,
            face_start: i as u32,
            face_count: 1,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: true,
        })
        .collect();
    let mut refs = Vec::new();
    for (cell_idx, cell) in cells.iter_mut().enumerate() {
        cell.portal_ref_start = refs.len() as u32;
        for (portal_idx, portal) in portals.iter().enumerate() {
            if portal.front_cell == cell_idx || portal.back_cell == cell_idx {
                refs.push(portal_idx as u32);
            }
        }
        cell.portal_ref_count = refs.len() as u32 - cell.portal_ref_start;
    }
    let has_portals = !portals.is_empty();
    LevelWorld::new_visibility_only(cells, refs, locator.0, locator.1, portals, has_portals)
        .expect("valid boxes world")
}

/// Locator node splitting on `axis` at `distance`; points at or above it go
/// to `front`.
pub(crate) fn axis_node(
    axis: usize,
    distance: f32,
    front: CellLocatorChild,
    back: CellLocatorChild,
) -> CellLocatorNodeData {
    let mut plane_normal = Vec3::ZERO;
    plane_normal[axis] = 1.0;
    CellLocatorNodeData {
        plane_normal,
        plane_distance: distance,
        front,
        back,
    }
}

/// Axis-aligned rectangle on the plane `axis = at`, spanning `lo..hi` on the
/// other two axes (in x, y, z order).
pub(crate) fn axis_rect(axis: usize, at: f32, lo: [f32; 2], hi: [f32; 2]) -> Vec<Vec3> {
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let corner = |a: f32, b: f32| {
        let mut p = Vec3::ZERO;
        p[axis] = at;
        p[u] = a;
        p[v] = b;
        p
    };
    vec![
        corner(lo[0], lo[1]),
        corner(hi[0], lo[1]),
        corner(hi[0], hi[1]),
        corner(lo[0], hi[1]),
    ]
}

pub(crate) fn portal(polygon: Vec<Vec3>, front_cell: usize, back_cell: usize) -> PortalData {
    PortalData {
        polygon,
        front_cell,
        back_cell,
    }
}

/// View-projection looking from `eye` toward `target`, runtime projection.
pub(crate) fn look_at_view_proj(eye: Vec3, target: Vec3) -> Mat4 {
    let look = (target - eye).normalize();
    let up = if look.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = glam::camera::rh::view::look_at_mat4(eye, target, up);
    let aspect = 16.0 / 9.0;
    let vfov = 2.0 * ((100f32.to_radians() / 2.0).tan() / aspect).atan();
    glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0) * view
}
