// Same-bake ghost-link capture controls and CPU-projected masks for the ghost capture test.
// See: context/lib/rendering_pipeline.md §7.8 (Frame capture), §7.9 · context/lib/testing_guide.md §3

use std::fs::File;
use std::path::Path;

use glam::{Mat4, Vec2, Vec3};
use postretro_level_format::ghost_links::GhostLinksSection;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::{SectionBlob, SectionId};

/// Quake units per metre (`build_pipeline.md` §Supported Map Formats).
const UNITS_PER_METRE: f32 = 39.37;

/// One fixed capture camera, in the scene file's own terms.
#[derive(Clone, Copy)]
pub(super) struct Camera {
    pub(super) eye: Vec3,
    pub(super) yaw_deg: f32,
    pub(super) pitch_deg: f32,
    pub(super) fov_deg: f32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Camera {
    pub(super) fn scene_json(&self) -> serde_json::Value {
        serde_json::json!({
            "position": self.eye.to_array(),
            "yaw_deg": self.yaw_deg,
            "pitch_deg": self.pitch_deg,
            "fov_deg": self.fov_deg,
        })
    }

    /// Capture's own view-projection (`capture::setup::capture_view_projection`).
    fn view_projection(&self) -> Mat4 {
        let aspect = self.width as f32 / self.height as f32;
        let vfov = 2.0 * ((self.fov_deg.to_radians() / 2.0).tan() / aspect).atan();
        let (yaw, pitch) = (self.yaw_deg.to_radians(), self.pitch_deg.to_radians());
        let direction = Vec3::new(
            -yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        );
        glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.01, 1000.0)
            * glam::camera::rh::view::look_at_mat4(self.eye, self.eye + direction, Vec3::Y)
    }

    /// Pixels whose centres any triangle of `positions`/`indices` covers, row-major.
    pub(super) fn mask(&self, positions: &[Vec3], indices: &[u32]) -> Vec<bool> {
        let (width, height) = (self.width, self.height);
        let view_projection = self.view_projection();
        let screen: Vec<Vec2> = positions
            .iter()
            .map(|&position| {
                let clip = view_projection * position.extend(1.0);
                assert!(
                    clip.w > 0.0,
                    "masked geometry must be in front of the camera"
                );
                let ndc = clip.truncate() / clip.w;
                Vec2::new(
                    (ndc.x + 1.0) * 0.5 * width as f32,
                    (1.0 - ndc.y) * 0.5 * height as f32,
                )
            })
            .collect();
        let mut mask = vec![false; (width * height) as usize];
        for triangle in indices.as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|corner| screen[triangle[corner] as usize]);
            let area = (b - a).perp_dot(c - a);
            if area.abs() < 1.0e-5 {
                continue;
            }
            let minimum = a.min(b).min(c).floor().max(Vec2::ZERO);
            let maximum = a
                .max(b)
                .max(c)
                .ceil()
                .min(Vec2::new(width as f32, height as f32));
            for y in minimum.y as u32..maximum.y as u32 {
                for x in minimum.x as u32..maximum.x as u32 {
                    let pixel = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let u = (b - pixel).perp_dot(c - pixel) / area;
                    let v = (c - pixel).perp_dot(a - pixel) / area;
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                        mask[(y * width + x) as usize] = true;
                    }
                }
            }
        }
        mask
    }
}

/// A Quake-space point (map units, Z up) in engine space (metres, Y up).
pub(super) fn quake_point(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(-y, z, -x) / UNITS_PER_METRE
}

/// The two triangles of an axis-aligned quad given by its four engine corners.
pub(super) fn quad(corners: [Vec3; 4]) -> (Vec<Vec3>, Vec<u32>) {
    (corners.to_vec(), vec![0, 1, 2, 0, 2, 3])
}

/// The tagged barrier's triangles carried into the other copy by `offset`, the
/// translation-only link mapping the fixture authors.
pub(super) fn mover_triangles(map: &Path, tag: &str, offset: Vec3) -> (Vec<Vec3>, Vec<u32>) {
    let world =
        postretro_level_loader::load_prl(&map.to_string_lossy()).expect("load capture fixture");
    let mover = world
        .kinematic_geometry
        .movers
        .iter()
        .find(|mover| mover.tags.iter().any(|t| t == tag))
        .unwrap_or_else(|| panic!("fixture contains the `{tag}` mover"));
    (
        mover
            .vertices
            .iter()
            .map(|vertex| Vec3::from_array(vertex.position) + mover.origin + offset)
            .collect(),
        mover.indices.clone(),
    )
}

/// Copy every section byte of `map` under the same content root, letting
/// `edit` rewrite one section's payload. No rebake can move the lighting.
fn rewrite(
    map: &Path,
    prefix: &str,
    mut edit: impl FnMut(u32, Vec<u8>) -> Vec<u8>,
) -> tempfile::TempPath {
    let mut source = File::open(map).expect("open capture PRL");
    let meta = postretro_level_format::read_container(&mut source).expect("read PRL container");
    let mut sections = Vec::new();
    for entry in &meta.sections {
        let data = postretro_level_format::read_section_data(&mut source, &meta, entry.section_id)
            .expect("read PRL section")
            .expect("listed section exists");
        sections.push(SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data: edit(entry.section_id, data),
        });
    }
    let mut output = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".prl")
        .tempfile_in(map.parent().expect("map parent"))
        .expect("reserve control PRL");
    postretro_level_format::write_prl(output.as_file_mut(), &sections).expect("write control PRL");
    output.into_temp_path()
}

/// The ghosts-disabled control: every link starts disarmed, everything else
/// byte-identical, so the capture draws no ghost.
pub(super) fn with_links_disarmed(map: &Path) -> tempfile::TempPath {
    let mut edited = 0;
    let control = rewrite(map, ".capture-ghosts-off-", |id, data| {
        if id != SectionId::GhostLinks as u32 {
            return data;
        }
        let mut section = GhostLinksSection::from_bytes(&data).expect("decode ghost links");
        for link in &mut section.links {
            link.enabled_on_spawn = false;
        }
        edited += 1;
        section.to_bytes()
    });
    assert_eq!(edited, 1, "the fixture carries one ghost-link section");
    control
}

/// The doubled-mover control: the `tag` mover's geometry plus a copy of it
/// moved by `offset` (mover-local engine metres) and retextured, everything
/// else byte-identical. Pushed less than its own thickness along the view, the
/// copy's front face lies inside the mover, behind the original's front face
/// and in front of everything else, so a ghost of it has a nearer and a
/// farther front-facing surface overlapping on screen, each its own texture.
pub(super) fn with_mover_doubled(map: &Path, tag: &str, offset: Vec3) -> tempfile::TempPath {
    with_mover_copy(map, ".capture-ghost-doubled-", tag, offset, true)
}

/// [`with_mover_doubled`]'s copy alone, in place of the mover's own geometry:
/// what the farther surface looks like with nothing in front of it.
pub(super) fn with_mover_copy_alone(map: &Path, tag: &str, offset: Vec3) -> tempfile::TempPath {
    with_mover_copy(map, ".capture-ghost-copy-", tag, offset, false)
}

fn with_mover_copy(
    map: &Path,
    prefix: &str,
    tag: &str,
    offset: Vec3,
    keep_original: bool,
) -> tempfile::TempPath {
    let mut edited = 0;
    let control = rewrite(map, prefix, |id, data| {
        if id != SectionId::KinematicGeometry as u32 {
            return data;
        }
        let mut section =
            KinematicGeometrySection::from_bytes(&data).expect("decode kinematic geometry");
        let mover = section
            .movers
            .iter_mut()
            .find(|mover| mover.tags.iter().any(|t| t == tag))
            .unwrap_or_else(|| panic!("fixture contains the `{tag}` mover"));
        let base = if keep_original {
            mover.vertices.len() as u32
        } else {
            0
        };
        let vertices: Vec<_> = mover
            .vertices
            .iter()
            .map(|vertex| {
                let mut copy = vertex.clone();
                copy.position = (Vec3::from_array(vertex.position) + offset).to_array();
                copy
            })
            .collect();
        let indices: Vec<u32> = mover.indices.iter().map(|index| index + base).collect();
        // Another of the level's textures (it has several), so the two
        // surfaces read apart.
        let faces: Vec<_> = mover
            .face_meta
            .iter()
            .map(|face| {
                let mut copy = face.clone();
                copy.texture_index = if face.texture_index == 0 { 1 } else { 0 };
                copy
            })
            .collect();
        if !keep_original {
            mover.vertices.clear();
            mover.indices.clear();
            mover.face_meta.clear();
        }
        mover.vertices.extend(vertices);
        mover.indices.extend(indices);
        mover.face_meta.extend(faces);
        edited += 1;
        section.to_bytes()
    });
    assert_eq!(
        edited, 1,
        "the fixture carries one kinematic geometry section"
    );
    control
}
