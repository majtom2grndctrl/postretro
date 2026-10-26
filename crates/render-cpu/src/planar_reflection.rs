// Planar water reflection, GPU-free: the mirror camera, candidate planes, selection and presets.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat4, Vec3, Vec4};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_level_loader::{CellData, LevelWorld};
use postretro_visibility::{
    Frustum, MirrorCropScratch, crop_mirror_view_proj, extract_frustum_planes,
};

use crate::fluid_surface::{
    face_area_centroid, face_normal,
    fog::FLUID_FOG_BAND,
    motion::{FLUID_RING_AMPLITUDE, FLUID_SHIMMER_AMPLITUDE},
    reflection::{FLUID_REFLECTION_RAY_MIN_Y, usable_probe_section},
    reflection_quality::{FluidReflectionQuality, reflection_target_extent},
    reflection_style::FLUID_REFLECTION_PIXEL_TILT,
};

/// Faces, and fluid triangles, within this many metres of one height share a plane.
pub const PLANE_HEIGHT_TOLERANCE: f32 = 0.01;
/// The oblique near plane sits this far beneath the water, so the waterline
/// keeps its own geometry in the mirror.
pub const PLANAR_CLIP_BIAS: f32 = 0.02;
/// A challenger must score above this multiple of the held plane to switch.
pub const PLANE_SELECTION_HYSTERESIS: f32 = 1.25;
/// The area floor, in m²: a plane whose total reflecting, probe-bound,
/// up-facing area (summed over its faces) falls below it never becomes a
/// candidate, so a puddle near the eye never outscores the lake. An engine
/// constant, never a map key, option or override (`rendering_pipeline.md`
/// §7.3a Selection). Placed from the baked fixtures' planes: the largest
/// puddle-scale plane is e24's floating block top, 7.9 m² (2.4 × 3.3 m),
/// and the smallest pool-scale one is reflection-probes' pool room, 23.8 m²
/// (4.9 × 4.9 m). 14 m², near their geometric mean, clears each by about
/// 1.7×.
pub const PLANAR_MIN_PLANE_AREA: f32 = 14.0;

/// Largest per-axis whole-texel displayed offset a pixel-style cell shows:
/// a shimmer plus a ring, each pinned at one texel per axis.
pub const MIRROR_CROP_MAX_OFFSET_TEXELS: i32 = FLUID_SHIMMER_AMPLITUDE + FLUID_RING_AMPLITUDE;

/// The mirror crop's pad: the largest angle between a pixel-style cell's
/// reflected ray and the flat water's ray through the same point, which is
/// the mirror eye's ray through it. Derivation:
/// - The tilt is `unit(u)·ox + unit(v)·oy` times `FLUID_REFLECTION_PIXEL_TILT`,
///   in the plane, so its tangent is at most `(|ox| + |oy|) · 0.05 = 4 · 0.05
///   = 0.2` (skewed texture axes reach the sum; orthogonal ones 2√2 · 0.05),
///   and the normal turns `atan(0.2) ≤ 0.2` rad.
/// - Mirroring about two normals `θ` apart differs by a rotation of `2θ`
///   (two half-turns compose to one), so the ray turns at most 0.4 rad.
/// - The horizon clamp (`FLUID_REFLECTION_RAY_MIN_Y`) moves a grazing flat
///   ray up to `asin(MIN_Y) ≤ π/2 · MIN_Y`, and moves the tilted ray no
///   further from the clamped flat one, so it adds at most `π · MIN_Y`.
///
/// ≈ 0.4031 rad (23.1°). Seen from the mirror eye, a sample at any depth
/// along such a ray lies within this angle of the eye's ray through its
/// water point, so it holds at every depth. Smooth style tilts nothing but
/// takes the same pad: the walk never depends on style.
pub const MIRROR_CROP_PAD_RADIANS: f32 =
    2.0 * (2 * MIRROR_CROP_MAX_OFFSET_TEXELS) as f32 * FLUID_REFLECTION_PIXEL_TILT
        + std::f32::consts::PI * FLUID_REFLECTION_RAY_MIN_Y;

/// One planar tier's engine-constant preset: the planar layer's per-axis
/// scale of the scene extent and the absolute-metre distance limits. `None`
/// leaves a distance unlimited. Never a player setting or map key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarPreset {
    pub scale: f32,
    /// Mirror reach: distance from the mirror eye to the reflected surface.
    pub d: Option<f32>,
    pub band_d: f32,
    /// Receiver reach: distance from the main eye to the water fragment.
    pub r: Option<f32>,
    pub band_r: f32,
}

/// Both planar tiers start at half scale with D and R off; the owner retunes
/// them from a motion read.
const PLANAR_PRESET_START: PlanarPreset = PlanarPreset {
    scale: 0.5,
    d: None,
    band_d: 4.0,
    r: None,
    band_r: 4.0,
};

const PLANAR_PRESETS: [(FluidReflectionQuality, PlanarPreset); 2] = [
    (FluidReflectionQuality::VeryHigh, PLANAR_PRESET_START),
    (FluidReflectionQuality::Ultra, PLANAR_PRESET_START),
];

/// The planar preset of `quality`: `Some` only at very high and ultra, the
/// tiers that run the planar layer.
pub fn planar_preset(quality: FluidReflectionQuality) -> Option<PlanarPreset> {
    PLANAR_PRESETS
        .iter()
        .find(|(tier, _)| *tier == quality)
        .map(|(_, preset)| preset.sanitized())
}

impl PlanarPreset {
    /// A supported scale (¼, ½ or 1), positive finite limits and bands.
    pub fn sanitized(self) -> Self {
        let scale = if self.scale.is_finite() {
            [0.25_f32, 0.5, 1.0]
                .into_iter()
                .min_by(|a, b| (self.scale - a).abs().total_cmp(&(self.scale - b).abs()))
                .expect("three supported planar scales")
        } else {
            0.5
        };
        let limit = |value: Option<f32>| value.filter(|v| v.is_finite() && *v > 0.0);
        let band = |value: f32| {
            if value.is_finite() {
                value.max(0.001)
            } else {
                4.0
            }
        };
        Self {
            scale,
            d: limit(self.d),
            band_d: band(self.band_d),
            r: limit(self.r),
            band_r: band(self.band_r),
        }
    }

    /// The planar layer's extent for a `scene` extent: rounded up, at least 1×1.
    pub fn target_extent(self, scene: (u32, u32)) -> (u32, u32) {
        reflection_target_extent(scene, self.sanitized().scale)
    }
}

/// Absolute-metre fade. An unlimited distance keeps full coverage.
pub fn planar_fade(distance: f32, limit: Option<f32>, band: f32) -> f32 {
    match limit {
        None => 1.0,
        Some(limit) if band > 0.0 => ((limit - distance) / band).clamp(0.0, 1.0),
        Some(limit) => f32::from(distance < limit),
    }
}

/// Install-time face geometry of one candidate plane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarFace {
    pub volume_index: u32,
    pub face_index: u32,
    pub vertices: Vec<Vec3>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub centroid: Vec3,
    pub area: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanarCandidate {
    pub h: f32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// Total area of `faces`, in m²: what the area floor reads.
    pub area: f32,
    pub faces: Vec<PlanarFace>,
}

#[derive(Debug, Clone, Default)]
pub struct PlanarCandidates {
    pub planes: Vec<PlanarCandidate>,
}

impl PlanarCandidates {
    /// The install-time candidate planes: flat up-facing faces of
    /// `reflecting_volume_indices` (the volumes whose fluid reflects) that
    /// bind a probe in `probes`, grouped by height. Planar keeps the
    /// reflecting gate's probe requirement per face: a face with no bound
    /// probe adds no area and no score, and a plane with none never forms.
    /// Planes under [`PLANAR_MIN_PLANE_AREA`] are dropped. Ascending by
    /// height, so a candidate's index is stable for one install.
    pub fn from_volumes(
        volumes: &[EnvironmentVolumeRecord],
        reflecting_volume_indices: &[u32],
        probes: Option<&ReflectionProbesSection>,
    ) -> Self {
        let Some(probes) = usable_probe_section(probes) else {
            return Self::default();
        };
        let mut planes = group_planes(volumes, |volume, face| {
            reflecting_volume_indices.contains(&volume) && face_binds_probe(probes, volume, face)
        });
        planes.retain(|plane| plane.area >= PLANAR_MIN_PLANE_AREA);
        Self { planes }
    }
}

/// Whether surface face `face` of volume `volume` binds a probe the section
/// holds. Bindings ascend by (volume, face), so this is one binary search.
fn face_binds_probe(section: &ReflectionProbesSection, volume: u32, face: u32) -> bool {
    section
        .bindings
        .binary_search_by(|binding| (binding.volume, binding.face).cmp(&(volume, face)))
        .is_ok_and(|index| (section.bindings[index].probe as usize) < section.probes.len())
}

/// Every flat up-facing face `keep` accepts, grouped into planes by height,
/// before the area floor. The floor's measurement reads it too.
fn group_planes(
    volumes: &[EnvironmentVolumeRecord],
    mut keep: impl FnMut(u32, u32) -> bool,
) -> Vec<PlanarCandidate> {
    let mut faces = Vec::new();
    for (volume_index, volume) in volumes.iter().enumerate() {
        for (face_index, face) in volume.surface_faces.iter().enumerate() {
            if !keep(volume_index as u32, face_index as u32) {
                continue;
            }
            let Some(normal) = face_normal(face).filter(|n| n.y > 0.9999) else {
                continue;
            };
            let vertices: Vec<Vec3> = face
                .vertices
                .iter()
                .map(|v| Vec3::from(v.position))
                .collect();
            let bounds_min = vertices
                .iter()
                .copied()
                .fold(Vec3::splat(f32::INFINITY), Vec3::min);
            let bounds_max = vertices
                .iter()
                .copied()
                .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
            if bounds_max.y - bounds_min.y > PLANE_HEIGHT_TOLERANCE {
                continue;
            }
            let first = vertices[0];
            let area = (1..vertices.len() - 1)
                .map(|i| {
                    (vertices[i] - first)
                        .cross(vertices[i + 1] - first)
                        .dot(normal)
                        * 0.5
                })
                .sum::<f32>()
                .abs();
            if !area.is_finite() || area <= 0.0 {
                continue;
            }
            faces.push(PlanarFace {
                volume_index: volume_index as u32,
                face_index: face_index as u32,
                centroid: face_area_centroid(face, normal).as_vec3(),
                vertices,
                bounds_min,
                bounds_max,
                area,
            });
        }
    }
    faces.sort_by(|a, b| a.centroid.y.total_cmp(&b.centroid.y));
    let mut planes: Vec<PlanarCandidate> = Vec::new();
    for face in faces {
        if let Some(plane) = planes
            .last_mut()
            .filter(|p| (p.h - face.centroid.y).abs() <= PLANE_HEIGHT_TOLERANCE)
        {
            plane.bounds_min = plane.bounds_min.min(face.bounds_min);
            plane.bounds_max = plane.bounds_max.max(face.bounds_max);
            plane.area += face.area;
            plane.faces.push(face);
        } else {
            planes.push(PlanarCandidate {
                h: face.centroid.y,
                bounds_min: face.bounds_min,
                bounds_max: face.bounds_max,
                area: face.area,
                faces: vec![face],
            });
        }
    }
    planes
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectedPlane {
    pub candidate_index: usize,
    pub h: f32,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub score: f32,
}

/// This frame's plane (`rendering_pipeline.md` §7.3a Selection), by plane
/// identity, from world geometry and the eye position; the main frustum
/// only decides which faces count. In order:
/// - **Filters, before scoring.** A plane competes only when `seeded` marks
///   it (a seed cell in this frame's camera reach, from
///   [`mark_seeded_planes`]; a missing entry reads as unseeded) and the eye
///   sits more than `FLUID_FOG_BAND` above it. The area floor applied at
///   install. A filtered plane scores zero.
/// - **Score.** Estimated projected solid angle of its faces that pass the
///   main frustum; ties keep the lower candidate index.
/// - **Hysteresis.** The held plane (`previous`) keeps selection while it
///   scores above zero and no challenger beats it by more than
///   [`PLANE_SELECTION_HYSTERESIS`] ([`challenger_switches`]). A held plane
///   scoring zero (filtered, or out of the frustum) yields that frame.
pub fn select_plane(
    candidates: &PlanarCandidates,
    eye: Vec3,
    main_vp: Mat4,
    seeded: &[bool],
    previous: Option<SelectedPlane>,
) -> Option<SelectedPlane> {
    let frustum = extract_frustum_planes(main_vp);
    let score = |index: usize, plane: &PlanarCandidate| -> f32 {
        if !seeded.get(index).copied().unwrap_or(false) || eye.y - plane.h <= FLUID_FOG_BAND {
            return 0.0;
        }
        plane
            .faces
            .iter()
            .filter(|face| bounds_in_frustum(face.bounds_min, face.bounds_max, &frustum))
            .map(|face| {
                let delta = eye - face.centroid;
                face.area * delta.y.abs() / delta.length_squared().max(0.0001).powf(1.5)
            })
            .sum()
    };
    let selected = |candidate_index: usize, plane: &PlanarCandidate, score: f32| SelectedPlane {
        candidate_index,
        h: plane.h,
        bounds_min: plane.bounds_min,
        bounds_max: plane.bounds_max,
        score,
    };
    let mut best: Option<SelectedPlane> = None;
    for (candidate_index, plane) in candidates.planes.iter().enumerate() {
        let score = score(candidate_index, plane);
        if score > 0.0 && best.is_none_or(|best| score > best.score) {
            best = Some(selected(candidate_index, plane, score));
        }
    }
    if let (Some(best), Some(previous)) = (best, previous)
        && let Some(plane) = candidates.planes.get(previous.candidate_index)
    {
        let held = score(previous.candidate_index, plane);
        if held > 0.0 && !challenger_switches(held, best.score) {
            return Some(selected(previous.candidate_index, plane, held));
        }
    }
    best
}

/// Whether a challenger scoring `challenger` takes selection from a held
/// plane scoring `held` above zero: only above [`PLANE_SELECTION_HYSTERESIS`]
/// times the held score. At or below it, the held plane stays.
pub fn challenger_switches(held: f32, challenger: f32) -> bool {
    challenger > held * PLANE_SELECTION_HYSTERESIS
}

/// Mark, one flag per candidate in `seeded` (reused across frames), each
/// plane with a seed cell in `reach`: this frame's camera reach, the walk's
/// fog reach on a walk path and the drawn set on a fallback. A seed cell is
/// one [`collect_mirror_seed_cells`] takes for that plane, so a selected
/// plane always seeds its mirror walk. Stops once every plane is marked.
pub fn mark_seeded_planes(
    candidates: &PlanarCandidates,
    reach: &[u32],
    world: &LevelWorld,
    eye: Vec3,
    r: Option<f32>,
    seeded: &mut Vec<bool>,
) {
    seeded.clear();
    seeded.resize(candidates.planes.len(), false);
    let mut unmarked = candidates.planes.len();
    for &id in reach {
        if unmarked == 0 {
            return;
        }
        let Some(cell) = world.cells.get(id as usize) else {
            continue;
        };
        for (plane, flag) in candidates.planes.iter().zip(seeded.iter_mut()) {
            if !*flag && seeds_plane(cell, plane.h, plane.bounds_min, plane.bounds_max, eye, r) {
                *flag = true;
                unmarked -= 1;
            }
        }
    }
}

fn bounds_in_frustum(min: Vec3, max: Vec3, frustum: &Frustum) -> bool {
    frustum.planes.iter().all(|plane| {
        let p = Vec3::new(
            if plane.normal.x >= 0.0 { max.x } else { min.x },
            if plane.normal.y >= 0.0 { max.y } else { min.y },
            if plane.normal.z >= 0.0 { max.z } else { min.z },
        );
        plane.normal.dot(p) + plane.dist >= 0.0
    })
}

/// Main-visible water cells seed a mirror walk whose apex may lie in solid.
/// `main_cells` may include empty reachable cells. Output storage is reused.
pub fn collect_mirror_seed_cells(
    main_cells: &[u32],
    world: &LevelWorld,
    plane: &SelectedPlane,
    eye: Vec3,
    r: Option<f32>,
    output: &mut Vec<u32>,
) {
    output.clear();
    for &id in main_cells {
        if world.cells.get(id as usize).is_some_and(|cell| {
            seeds_plane(cell, plane.h, plane.bounds_min, plane.bounds_max, eye, r)
        }) {
            output.push(id);
        }
    }
    output.sort_unstable();
    output.dedup();
}

/// Whether `cell` seeds the mirror of the plane at height `h` over
/// `bounds_min..bounds_max`: non-solid, interior, straddling the water
/// within the fog band, overlapping the plane in x and z, and, with a
/// receiver reach `r`, holding water within `r` of the eye.
fn seeds_plane(
    cell: &CellData,
    h: f32,
    bounds_min: Vec3,
    bounds_max: Vec3,
    eye: Vec3,
    r: Option<f32>,
) -> bool {
    if cell.is_solid
        || cell.is_exterior
        || cell.bounds_max.y < h - FLUID_FOG_BAND
        || cell.bounds_min.y > h + FLUID_FOG_BAND
        || cell.bounds_max.x < bounds_min.x
        || cell.bounds_min.x > bounds_max.x
        || cell.bounds_max.z < bounds_min.z
        || cell.bounds_min.z > bounds_max.z
    {
        return false;
    }
    let water_min = Vec3::new(
        cell.bounds_min.x.max(bounds_min.x),
        h,
        cell.bounds_min.z.max(bounds_min.z),
    );
    let water_max = Vec3::new(
        cell.bounds_max.x.min(bounds_max.x),
        h,
        cell.bounds_max.z.min(bounds_max.z),
    );
    !r.is_some_and(|r| eye.distance_squared(eye.clamp(water_min, water_max)) > r * r)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MirrorCamera {
    pub eye: Vec3,
    pub cull_vp: Mat4,
    pub raster_vp: Mat4,
    /// World-space inward clip half-space: dot(plane, vec4(p, 1)) >= 0.
    pub clip_plane: Vec4,
    pub h: f32,
}

impl MirrorCamera {
    /// `projection` is the main camera's finite RH DirectX perspective.
    /// Cull depth uses D; raster depth keeps the main far distance.
    pub fn new(eye: Vec3, view: Mat4, projection: Mat4, h: f32, d: Option<f32>) -> Option<Self> {
        if !eye.is_finite()
            || !view.is_finite()
            || !projection.is_finite()
            || !h.is_finite()
            || eye.y - h <= FLUID_FOG_BAND
        {
            return None;
        }
        let reflection = Mat4::from_cols(
            Vec4::X,
            -Vec4::Y,
            Vec4::Z,
            Vec4::new(0.0, 2.0 * h, 0.0, 1.0),
        );
        let mirror_view = view * reflection;
        let clip_plane = Vec4::new(0.0, 1.0, 0.0, -(h - PLANAR_CLIP_BIAS));
        let camera_plane = mirror_view.inverse().transpose() * clip_plane;
        // Lengyel's oblique near plane: for [0,1] depth it replaces row 2.
        let corner = Vec4::new(
            if camera_plane.x >= 0.0 { 1.0 } else { -1.0 },
            if camera_plane.y >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            1.0,
        );
        let denominator = camera_plane.dot(projection.inverse() * corner);
        if !denominator.is_finite() || denominator <= 1.0e-6 {
            return None;
        }
        let near_row = camera_plane / denominator;
        let mut oblique = projection;
        oblique.x_axis.z = near_row.x;
        oblique.y_axis.z = near_row.y;
        oblique.z_axis.z = near_row.z;
        oblique.w_axis.z = near_row.w;
        let mut cull_projection = projection;
        if let Some(d) = d.filter(|d| d.is_finite() && *d > 0.0) {
            let near = projection.w_axis.z / projection.z_axis.z;
            let far = d.max(near + 0.001);
            cull_projection.z_axis.z = far / (near - far);
            cull_projection.w_axis.z = near * far / (near - far);
        }
        // The mirror flips handedness; flipping x restores the main winding.
        let flip_x = Mat4::from_diagonal(Vec4::new(-1.0, 1.0, 1.0, 1.0));
        Some(Self {
            eye: reflection.transform_point3(eye),
            cull_vp: cull_projection * mirror_view,
            raster_vp: flip_x * oblique * mirror_view,
            clip_plane,
            h,
        })
    }

    /// The cull frustum's six inward planes (left, right, bottom, top, near,
    /// far) as `dot(plane, vec4(p, 1)) >= 0`, for [0, 1] depth. Unnormalized
    /// and allocation-free, for per-leaf tests on the frame path.
    pub fn cull_planes(&self) -> [Vec4; 6] {
        view_proj_planes(self.cull_vp)
    }

    /// This mirror with its cull view-projection cropped to `plane`
    /// (`rendering_pipeline.md` §7.3a Visibility): the on-screen part of each
    /// face that passes the main frustum (`main_vp`), padded by
    /// [`MIRROR_CROP_PAD_RADIANS`]. Only faces in view count, so a coplanar
    /// pool in another room never widens it. The plane is fixed under the
    /// reflection, so its points project through `cull_vp` where the main
    /// view shows them. `raster_vp` and `clip_plane` stay as they are: the
    /// crop culls, it never moves a pixel. `None` when no face keeps any part
    /// on screen; then no mirror sample lands anywhere.
    pub fn cropped_to_plane(
        &self,
        plane: &PlanarCandidate,
        main_vp: Mat4,
        scratch: &mut MirrorCropScratch,
    ) -> Option<Self> {
        let main = view_proj_planes(main_vp);
        let faces = plane
            .faces
            .iter()
            .filter(|face| aabb_in_planes(face.bounds_min, face.bounds_max, &main))
            .map(|face| face.vertices.as_slice());
        let cull_vp = crop_mirror_view_proj(
            self.cull_vp,
            self.eye,
            faces,
            MIRROR_CROP_PAD_RADIANS,
            scratch,
        )?;
        Some(Self { cull_vp, ..*self })
    }
}

/// A [0, 1]-depth view-projection's six inward planes (left, right, bottom,
/// top, near, far) as `dot(plane, vec4(p, 1)) >= 0`, unnormalized.
fn view_proj_planes(m: Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
}

/// Whether an AABB reaches inside every plane of `planes` (the p-vertex
/// test): conservative, so a box straddling a plane passes.
pub fn aabb_in_planes(min: Vec3, max: Vec3, planes: &[Vec4; 6]) -> bool {
    planes.iter().all(|plane| {
        let p = Vec3::new(
            if plane.x >= 0.0 { max.x } else { min.x },
            if plane.y >= 0.0 { max.y } else { min.y },
            if plane.z >= 0.0 { max.z } else { min.z },
        );
        plane.truncate().dot(p) + plane.w >= 0.0
    })
}

#[cfg(test)]
#[path = "planar_reflection_tests.rs"]
mod tests;
