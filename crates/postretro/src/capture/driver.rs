// Synchronous, world-only offscreen frame-capture driver.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use image::ImageEncoder as _;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_visibility::CameraCullVisibility;

use crate::camera;
use crate::render::{ClearColor, LevelGeometry, Renderer, level_world_to_geometry};
use crate::startup::session::content_root_from_map;
use crate::startup::worker::derive_prm_root_dev_layout;

use super::scene::{CameraPose, ForcedAnimLight, parse_scene};

/// Portal-walk capture controls diagnostics only; capture has no diagnostic
/// consumer, so avoid allocating a one-frame trace.
const CAPTURE_PORTAL_WALK: bool = false;
const MAX_UNIQUE_FILE_ATTEMPTS: usize = 1024;
static NEXT_UNIQUE_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// Entry point wired from `startup::build_session`. Capture performs all work
/// synchronously and always terminates, preventing winit startup on both success
/// and failure.
pub(crate) fn run_capture(scene_arg: Option<&str>) -> ! {
    match run_capture_inner(scene_arg) {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("[Capture] {err:#}");
            std::process::exit(1);
        }
    }
}

fn run_capture_inner(scene_arg: Option<&str>) -> Result<()> {
    let scene_arg =
        scene_arg.ok_or_else(|| anyhow!("`--capture` requires a scene JSON path argument"))?;
    let scene_path = Path::new(scene_arg);
    let text = fs::read_to_string(scene_path)
        .with_context(|| format!("failed to read capture scene `{scene_arg}`"))?;
    let scene = parse_scene(&text)?;

    let map_path = Path::new(&scene.map);
    if !map_path.is_file() {
        bail!("map not found: `{}`", scene.map);
    }
    let output_path = Path::new(&scene.output);
    reject_output_source_aliases(output_path, map_path, scene_path)?;
    preflight_output_path(output_path)?;

    // Load synchronously: capture creates no worker thread or event loop.
    let mut world = postretro_level_loader::load_prl(&scene.map)
        .with_context(|| format!("failed to load `{}`", scene.map))?;

    let [width, height] = scene.resolution;
    let mut renderer = Renderer::new_offscreen(width, height)
        .context("failed to initialize offscreen frame capture renderer")?;

    let texture_materials = derive_texture_materials(&world.texture_names);
    let content_root = content_root_from_map(Some(&scene.map));
    let prm_cache_root = derive_prm_root_dev_layout(&content_root);
    renderer.install_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        &prm_cache_root,
        &texture_materials,
    );
    renderer.normalize_world_uvs(&mut world);
    let (static_lights, static_entity_shadow_lights) =
        capture_static_lights_and_shadow_selection(&world.lights, &world.entity_shadow_lights);
    let geometry = LevelGeometry {
        lights: &static_lights,
        light_influences: &[],
        entity_shadow_lights: &static_entity_shadow_lights,
        ..level_world_to_geometry(&world, &texture_materials)
    };
    renderer.install_level_geometry(&geometry);
    install_forced_active_animation_descriptors(
        &mut renderer,
        &world.lights,
        scene.force_active.as_deref(),
    )?;

    let eye = Vec3::from_array(scene.camera.position);
    let view_proj = capture_view_projection(&scene.camera, width, height);
    let mut scratch = Vec::new();
    let (visibility, _frustum) = postretro_visibility::determine_visible_cells(
        eye,
        view_proj,
        &world,
        &[],
        CAPTURE_PORTAL_WALK,
        &mut scratch,
    );
    let visible_cells = visibility.visible_cells;
    let fog_reachable = visibility.fog_reachable;
    let stats = visibility.stats;
    let light_reachable_cell_mask = light_reachable_cell_mask(&world, &fog_reachable);
    let reachable_cell_aabbs = reachable_cell_aabbs(&world, &fog_reachable);

    let rgba = renderer.capture_frame_indirect(
        CameraCullVisibility {
            cells: &visible_cells,
            path: stats.path,
        },
        &light_reachable_cell_mask,
        &reachable_cell_aabbs,
        &fog_reachable,
        Some(stats.camera_cell),
        view_proj,
        eye,
        &[],
        ClearColor {
            r: 0.05,
            g: 0.05,
            b: 0.08,
            a: 1.0,
        },
        true,
    )?;

    // `scene_color` readback is already RGBA8 sRGB. Write only after all
    // rendering succeeded, so invalid input or GPU failures never touch output.
    write_capture_png(output_path, &rgba, width, height)?;

    Ok(())
}

/// Seed capture-only authored active states after the level install has restored
/// the baked descriptor mirror. `capture_frame_indirect` flushes these writes
/// in its first `update_per_frame_uniforms` call.
fn install_forced_active_animation_descriptors(
    renderer: &mut Renderer,
    lights: &[postretro_level_loader::MapLight],
    forced_lights: Option<&[ForcedAnimLight]>,
) -> Result<()> {
    for (slot, radiance) in resolve_forced_active_animation_slots(lights, forced_lights)? {
        renderer
            .write_animated_compose_descriptor(slot, &forced_active_animation_descriptor(radiance));
    }
    Ok(())
}

/// Resolve authored tags against the complete map-light list. Capture's
/// static-only forward-light filter has a compacted index space, while
/// `animated_slot` names the independently indexed SH compose descriptor.
fn resolve_forced_active_animation_slots(
    lights: &[postretro_level_loader::MapLight],
    forced_lights: Option<&[ForcedAnimLight]>,
) -> Result<Vec<(u32, [f32; 3])>> {
    let Some(forced_lights) = forced_lights else {
        return Ok(Vec::new());
    };

    // Keying writes by slot both deduplicates multi-light tag matches and gives
    // the renderer a stable write order independent of `world.lights` order.
    let mut slot_radiance = BTreeMap::new();
    for forced in forced_lights {
        let mut tag_found = false;
        let mut animated_slot_found = false;
        for light in lights {
            if !light.tags.iter().any(|tag| tag == &forced.tag) {
                continue;
            }
            tag_found = true;
            let Some(slot) = light.animated_slot else {
                continue;
            };
            animated_slot_found = true;
            match slot_radiance.entry(slot) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(forced.radiance);
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if *entry.get() == forced.radiance => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    bail!(
                        "force_active tag `{}` resolves to an animated descriptor with conflicting radiance",
                        forced.tag
                    );
                }
            }
        }
        if !tag_found {
            bail!(
                "force_active tag `{}` does not match a map light",
                forced.tag
            );
        }
        if !animated_slot_found {
            bail!(
                "force_active tag `{}` does not match an animated map light",
                forced.tag
            );
        }
    }

    Ok(slot_radiance.into_iter().collect())
}

/// Build the active/no-curve compose descriptor through the shared descriptor
/// packer. With `animation: None` and `active_without_animation: true`, the
/// radiance lands in `base_color` and `color_count` remains zero.
fn forced_active_animation_descriptor(
    radiance: [f32; 3],
) -> [u8; postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE] {
    let component = LightComponent {
        origin: [0.0; 3],
        light_type: LightKind::Point,
        intensity: 1.0,
        color: radiance,
        falloff_model: FalloffKind::Linear,
        falloff_range: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        is_dynamic: false,
        animated_slot: None,
        follow_transform: false,
        carrier: None,
        animation: None,
    };
    crate::scripting_systems::light_bridge::pack_animation_descriptor(
        &component, 0, 0, radiance, true, None,
    )
}

/// Preserve capture's static-only light input while translating global PRL
/// selection indices into the same compact static-light index space. Keep one
/// output selection entry per input entry so shadowmask channels stay aligned.
fn capture_static_lights_and_shadow_selection(
    lights: &[postretro_level_loader::MapLight],
    entity_shadow_lights: &[u32],
) -> (Vec<postretro_level_loader::MapLight>, Vec<u32>) {
    let mut global_to_static = vec![u32::MAX; lights.len()];
    let mut static_lights = Vec::with_capacity(lights.len());
    for (global_index, light) in lights.iter().enumerate() {
        if light.is_dynamic {
            continue;
        }
        global_to_static[global_index] = static_lights.len() as u32;
        static_lights.push(light.clone());
    }

    let static_entity_shadow_lights = entity_shadow_lights
        .iter()
        .map(|&global_index| {
            global_to_static
                .get(global_index as usize)
                .copied()
                .unwrap_or(u32::MAX)
        })
        .collect();

    (static_lights, static_entity_shadow_lights)
}

fn derive_texture_materials(
    texture_names: &[String],
) -> Vec<postretro_render_data::material::Material> {
    let mut warned = HashSet::new();
    texture_names
        .iter()
        .map(|name| {
            let warned_count = warned.len();
            let material = postretro_render_data::material::derive_material(name, &mut warned);
            let prefix = postretro_render_data::material::parse_prefix(name);
            if material == postretro_render_data::material::Material::Default
                && !prefix.is_empty()
                && warned.len() > warned_count
            {
                log::warn!(
                    "[Material] Unknown prefix '{}' in texture '{}' — using default material",
                    prefix,
                    name,
                );
            }
            material
        })
        .collect()
}

/// Build the static capture camera directly so the scene FOV is honored instead
/// of using `RenderCamera::new`, whose projection always uses `camera::HFOV`.
fn capture_view_projection(camera: &CameraPose, width: u32, height: u32) -> Mat4 {
    let aspect = width as f32 / height as f32;
    let fov = camera.fov_deg.to_radians();
    let vfov = 2.0 * ((fov / 2.0).tan() / aspect).atan();
    let yaw = camera.yaw_deg.to_radians();
    let pitch = camera.pitch_deg.to_radians();
    let look_dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let eye = Vec3::from_array(camera.position);
    Mat4::perspective_rh(vfov, aspect, camera::NEAR, camera::FAR)
        * Mat4::look_at_rh(eye, eye + look_dir, Vec3::Y)
}

/// Mirror `App::redraw`: an empty fog-reachable list is the DrawAll sentinel,
/// so an empty mask keeps every cell-assigned light eligible.
fn light_reachable_cell_mask(
    world: &postretro_level_loader::LevelWorld,
    fog_reachable: &[u32],
) -> Vec<bool> {
    if fog_reachable.is_empty() {
        return Vec::new();
    }
    let mut mask = vec![false; world.cell_count()];
    for &id in fog_reachable {
        let index = id as usize;
        if index < mask.len() {
            mask[index] = true;
        }
    }
    mask
}

/// Mirror `App::redraw`: shadow eligibility follows the wider fog/light
/// reachability set, including empty but portal-reachable cells.
fn reachable_cell_aabbs(
    world: &postretro_level_loader::LevelWorld,
    fog_reachable: &[u32],
) -> Vec<(Vec3, Vec3)> {
    if fog_reachable.is_empty() {
        return Vec::new();
    }
    fog_reachable
        .iter()
        .filter_map(|&id| world.cells.get(id as usize))
        .map(|cell| (cell.bounds_min, cell.bounds_max))
        .collect()
}

/// Reject an output that resolves to either capture input. Existing paths are
/// canonicalized, so relative, absolute, and symlink spellings compare by the
/// file they name rather than by their source text.
fn reject_output_source_aliases(output: &Path, map: &Path, scene: &Path) -> Result<()> {
    let output_key = path_alias_key(output)?;
    for (label, source) in [("map", map), ("scene JSON", scene)] {
        if output_key == path_alias_key(source)? {
            bail!(
                "output path must not alias the capture {label}: `{}`",
                output.display()
            );
        }
    }
    Ok(())
}

fn path_alias_key(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let absolute = if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir()
                    .context("failed to resolve current directory for capture paths")?
                    .join(path)
            };
            let mut normalized = PathBuf::new();
            for component in absolute.components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    other => normalized.push(other.as_os_str()),
                }
            }
            // A missing intermediate component (for example, `nested/..`) can
            // make the initial canonicalization fail even when the normalized
            // path names an existing source. Canonicalize that result when
            // possible so it compares consistently with source paths.
            match fs::canonicalize(&normalized) {
                Ok(canonical) => Ok(canonical),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(normalized),
                Err(err) => {
                    Err(err).with_context(|| format!("failed to resolve `{}`", path.display()))
                }
            }
        }
        Err(err) => Err(err).with_context(|| format!("failed to resolve `{}`", path.display())),
    }
}

/// Verify the atomic publication operations before renderer initialization.
/// Existing targets must be regular files; the parent must support sibling
/// creation and replacement rename.
fn preflight_output_path(output: &Path) -> Result<()> {
    if output.as_os_str().is_empty() {
        bail!("output path is empty");
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent)
        .with_context(|| format!("output parent does not exist: `{}`", parent.display()))?;
    if !parent_metadata.is_dir() {
        bail!("output parent is not a directory: `{}`", parent.display());
    }

    let output_exists = match fs::symlink_metadata(output) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("output path is not a regular file: `{}`", output.display());
        }
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect output `{}`", output.display()));
        }
    };
    probe_atomic_publication(parent, output, output_exists)?;
    Ok(())
}

fn probe_atomic_publication(parent: &Path, output: &Path, replacing: bool) -> Result<()> {
    let mut source = create_unique_sibling_file(parent, "write-probe-source")
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    let mut destination = create_unique_sibling_file(parent, "write-probe-destination")
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    source
        .file_mut()
        .write_all(&[0])
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    source.close();
    destination.close();
    if !replacing {
        fs::remove_file(destination.path())
            .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    }

    fs::rename(source.path(), destination.path()).with_context(|| {
        format!(
            "output parent does not support atomic publication beside `{}`",
            output.display()
        )
    })?;
    source.persist();
    fs::remove_file(destination.path()).with_context(|| {
        format!(
            "failed to remove output publication probe `{}`",
            destination.path().display()
        )
    })?;
    destination.persist();
    Ok(())
}

/// Encode the complete PNG into a sibling file, then atomically replace the
/// requested output. The sibling placement makes the rename atomic and keeps
/// a failed encode from truncating an existing capture.
fn write_capture_png(output: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    let mut temporary = create_capture_temp_file(output)?;
    let temporary_path = temporary.path().to_owned();
    {
        let file = temporary.file_mut();
        image::codecs::png::PngEncoder::new(&mut *file)
            .write_image(rgba, width, height, image::ColorType::Rgba8.into())
            .with_context(|| format!("failed to encode capture PNG `{}`", output.display()))?;
        file.flush().with_context(|| {
            format!("failed to flush capture PNG `{}`", temporary_path.display())
        })?;
        file.sync_all().with_context(|| {
            format!("failed to sync capture PNG `{}`", temporary_path.display())
        })?;
    }
    temporary.close();

    fs::rename(temporary.path(), output)
        .with_context(|| format!("failed to finalize capture PNG `{}`", output.display()))?;
    temporary.persist();
    Ok(())
}

/// A newly-created capture file that removes itself unless it is renamed into
/// place successfully.
struct CaptureTempFile {
    path: Option<PathBuf>,
    file: Option<File>,
}

impl CaptureTempFile {
    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("temporary capture file path exists until persisted")
    }

    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary capture file remains open until finalization")
    }

    fn close(&mut self) {
        drop(self.file.take());
    }

    fn persist(mut self) {
        self.path = None;
    }
}

impl Drop for CaptureTempFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn create_capture_temp_file(output: &Path) -> Result<CaptureTempFile> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut label = OsString::from("capture-");
    label.push(
        output
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("capture")),
    );
    create_unique_sibling_file(parent, &label).with_context(|| {
        format!(
            "failed to create temporary capture PNG beside `{}`",
            output.display()
        )
    })
}

fn create_unique_sibling_file(
    parent: &Path,
    label: impl AsRef<std::ffi::OsStr>,
) -> Result<CaptureTempFile> {
    for _ in 0..MAX_UNIQUE_FILE_ATTEMPTS {
        let path = unique_sibling_path(parent, label.as_ref());
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok(CaptureTempFile {
                    path: Some(path),
                    file: Some(file),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to create `{}`", path.display()));
            }
        }
    }
    bail!("could not create a unique file in `{}`", parent.display())
}

fn unique_sibling_path(parent: &Path, label: &std::ffi::OsStr) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".postretro-capture-");
    name.push(label);
    name.push(format!(
        "-{}-{timestamp:032x}-{sequence:016x}.tmp",
        std::process::id()
    ));
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_view_projection_uses_pitch_in_look_direction() {
        let camera = CameraPose {
            position: [0.0, 0.0, 0.0],
            yaw_deg: 0.0,
            pitch_deg: 30.0,
            fov_deg: 100.0,
        };
        let view_proj = capture_view_projection(&camera, 1280, 720);
        let expected = Mat4::perspective_rh(
            2.0 * ((camera.fov_deg.to_radians() / 2.0).tan() / (1280.0 / 720.0)).atan(),
            1280.0 / 720.0,
            camera::NEAR,
            camera::FAR,
        ) * Mat4::look_at_rh(
            Vec3::ZERO,
            Vec3::new(
                0.0,
                30.0_f32.to_radians().sin(),
                -30.0_f32.to_radians().cos(),
            ),
            Vec3::Y,
        );
        assert_mat4_approx_eq(view_proj, expected, 1.0e-6);
    }

    #[test]
    fn capture_view_projection_honors_scene_fov() {
        let narrow = CameraPose {
            position: [0.0, 0.0, 0.0],
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            fov_deg: 60.0,
        };
        let wide = CameraPose {
            fov_deg: 130.0,
            ..narrow.clone()
        };
        let narrow_projection = capture_view_projection(&narrow, 1280, 720);
        let wide_projection = capture_view_projection(&wide, 1280, 720);
        assert_ne!(narrow_projection, wide_projection);
    }

    // Regression: passing the full light list fixed shadowmask indexing but
    // also introduced dynamic lights into captures that were static-only.
    #[test]
    fn capture_lights_remain_static_only_and_remap_shadow_selection() {
        let lights = [
            test_light(true, 1.0),
            test_light(false, 2.0),
            test_light(true, 3.0),
            test_light(false, 4.0),
        ];

        let (captured, selection) =
            capture_static_lights_and_shadow_selection(&lights, &[3, 0, 1, 99]);

        assert_eq!(captured.len(), 2, "capture must retain two static lights");
        assert!(
            (captured[0].intensity - 2.0).abs() < f32::EPSILON
                && (captured[1].intensity - 4.0).abs() < f32::EPSILON,
            "capture must retain the pre-change static-only compact order",
        );
        assert!(captured.iter().all(|light| !light.is_dynamic));
        assert_eq!(
            selection,
            vec![1, u32::MAX, 0, u32::MAX],
            "selection order must stay channel-aligned while indices move into compact static space",
        );
    }

    #[test]
    fn absent_force_active_leaves_baked_descriptors_unmodified() {
        let writes = resolve_forced_active_animation_slots(&[test_light(false, 1.0)], None)
            .expect("absent force_active must not fail");
        assert!(
            writes.is_empty(),
            "without authored state, capture must leave install_level_geometry's baked descriptors intact"
        );
    }

    #[test]
    fn force_active_resolves_every_matching_animated_slot_once_in_stable_order() {
        let mut first = test_light(true, 1.0);
        first.tags = vec!["alarm_light".into()];
        first.animated_slot = Some(7);
        let mut second = test_light(false, 2.0);
        second.tags = vec!["alarm_light".into()];
        second.animated_slot = Some(3);
        let mut duplicate_slot = test_light(false, 3.0);
        duplicate_slot.tags = vec!["alarm_light".into()];
        duplicate_slot.animated_slot = Some(7);
        let forced = [ForcedAnimLight {
            tag: "alarm_light".into(),
            radiance: [4.0, 0.0, 0.0],
        }];

        let forward = resolve_forced_active_animation_slots(
            &[first.clone(), second.clone(), duplicate_slot.clone()],
            Some(&forced),
        )
        .expect("tag must resolve");
        let reversed =
            resolve_forced_active_animation_slots(&[duplicate_slot, second, first], Some(&forced))
                .expect("tag must resolve after map-light reordering");

        let expected = vec![(3, [4.0, 0.0, 0.0]), (7, [4.0, 0.0, 0.0])];
        assert_eq!(forward, expected);
        assert_eq!(reversed, expected);
    }

    #[test]
    fn force_active_rejects_unknown_map_light_tag() {
        let forced = [ForcedAnimLight {
            tag: "unknown_light".into(),
            radiance: [4.0, 0.0, 0.0],
        }];
        let err = resolve_forced_active_animation_slots(&[test_light(false, 1.0)], Some(&forced))
            .expect_err("unknown tag must not silently skip");
        assert!(err.to_string().contains("unknown_light"));
    }

    #[test]
    fn forced_active_descriptor_uses_shared_active_without_animation_layout() {
        let descriptor = forced_active_animation_descriptor([4.0, 0.5, 0.25]);
        assert_eq!(
            f32::from_ne_bytes(descriptor[0..4].try_into().unwrap()),
            1.0
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[12..16].try_into().unwrap()),
            0,
            "the authored instant has no brightness curve samples"
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[16..20].try_into().unwrap()),
            4.0
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[20..24].try_into().unwrap()),
            0.5
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[24..28].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[32..36].try_into().unwrap()),
            0,
            "the authored instant has no color curve samples"
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[36..40].try_into().unwrap()),
            1
        );
    }

    #[test]
    fn output_must_not_alias_map_or_scene_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let map = directory.path().join("level.prl");
        let scene = directory.path().join("scene.json");
        fs::write(&map, b"map").expect("map fixture");
        fs::write(&scene, b"scene").expect("scene fixture");

        let map_alias = directory.path().join("nested").join("..").join("level.prl");
        assert!(reject_output_source_aliases(&map_alias, &map, &scene).is_err());
        assert!(reject_output_source_aliases(&scene, &map, &scene).is_err());
        assert!(
            reject_output_source_aliases(&directory.path().join("new.png"), &map, &scene).is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_symlink_alias_and_non_regular_target_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let map = directory.path().join("level.prl");
        let scene = directory.path().join("scene.json");
        let output = directory.path().join("capture.png");
        fs::write(&map, b"map").expect("map fixture");
        fs::write(&scene, b"scene").expect("scene fixture");
        symlink(&map, &output).expect("output symlink");

        assert!(reject_output_source_aliases(&output, &map, &scene).is_err());
        assert!(preflight_output_path(&output).is_err());
    }

    #[test]
    fn output_preflight_exercises_atomic_publication_without_leaking_probes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("capture.png");

        preflight_output_path(&output).expect("new output preflight");
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            0
        );

        fs::write(&output, b"existing").expect("existing output");
        preflight_output_path(&output).expect("replacement output preflight");
        assert_eq!(
            fs::read(&output).expect("existing output intact"),
            b"existing"
        );
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            1
        );
    }

    #[test]
    fn unique_sibling_creation_is_not_limited_to_sixteen_candidates() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let files: Vec<_> = (0..64)
            .map(|_| create_unique_sibling_file(directory.path(), "collision-test").expect("file"))
            .collect();
        let paths: HashSet<_> = files.iter().map(|file| file.path().to_owned()).collect();
        assert_eq!(paths.len(), files.len());
        drop(files);
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            0
        );
    }

    #[test]
    fn capture_png_replaces_existing_output_without_leaving_temp_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("capture.png");
        fs::write(&output, b"old capture").expect("existing capture");

        write_capture_png(&output, &[1, 2, 3, 4], 1, 1).expect("write capture");

        let decoded = image::open(&output).expect("decode capture").to_rgba8();
        assert_eq!(decoded.as_raw(), &[1, 2, 3, 4]);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read capture directory")
                .count(),
            1,
            "successful capture removes its temporary sibling"
        );
    }

    fn test_light(is_dynamic: bool, intensity: f32) -> postretro_level_loader::MapLight {
        postretro_level_loader::MapLight {
            origin: [0.0; 3],
            light_type: postretro_level_loader::LightType::Point,
            intensity,
            color: [1.0; 3],
            falloff_model: postretro_level_loader::FalloffModel::Linear,
            falloff_range: 8.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: Vec::new(),
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
        }
    }

    fn assert_mat4_approx_eq(actual: Mat4, expected: Mat4, epsilon: f32) {
        for (actual, expected) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!(
                (actual - expected).abs() <= epsilon,
                "matrix values differ: {actual} != {expected}"
            );
        }
    }
}
