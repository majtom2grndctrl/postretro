// Loading a camera-route pose file for playback, and matching it to a level.
// See: context/lib/rendering_pipeline.md §12

use std::path::Path;

use postretro_camera_route::{PoseFile, bake_token, parse_pose_file};

use crate::camera::{MAX_FOV_DEG, MIN_FOV_DEG};

/// Whether a route's bake was shown to be the level's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Identity {
    /// The id-51 hash and walk epoch match.
    Proven,
    /// The level has no id-51 hash, so nothing could be compared.
    Unproven,
}

impl Identity {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Proven => "proven",
            Self::Unproven => "unproven",
        }
    }
}

/// Reads and checks a pose file: readable, well formed, at least one pose,
/// a bake line carrying `id51` and `walk_epoch`, and a horizontal FOV inside
/// the engine's band, never clamped. The error names the file and reason.
pub(crate) fn load_route(path: &Path) -> Result<PoseFile, String> {
    let named = |reason: String| format!("camera route {}: {reason}", path.display());
    let text = std::fs::read_to_string(path).map_err(|err| named(format!("unreadable: {err}")))?;
    let file = parse_pose_file(&text).map_err(|err| named(format!("malformed at {err}")))?;
    if file.header.id51().is_none() || file.header.walk_epoch().is_none() {
        return Err(named(format!(
            "malformed `bake` line: needs id51 and walk_epoch, found {:?}",
            file.header.bake
        )));
    }
    let hfov = file.header.projection.hfov_degrees;
    if !(MIN_FOV_DEG..=MAX_FOV_DEG).contains(&hfov) {
        return Err(named(format!(
            "horizontal FOV {hfov}° lies outside the engine's {MIN_FOV_DEG}°..={MAX_FOV_DEG}°"
        )));
    }
    Ok(file)
}

/// Matches a route's bake to the loaded level's (`bake_identity`'s string)
/// on the id-51 hash and walk epoch only, the walk's own same-bake check:
/// size, modification time and path say only which file was read. A level
/// with no id-51 hash cannot be compared and is accepted unproven.
pub(crate) fn match_level(file: &PoseFile, level_bake: &str) -> Result<Identity, String> {
    let level_id51 = bake_token(level_bake, "id51");
    let level_epoch = bake_token(level_bake, "walk_epoch");
    let route_id51 = file.header.id51();
    let route_epoch = bake_token(&file.header.bake, "walk_epoch");
    if level_epoch != route_epoch {
        return Err(format!(
            "walk epoch {} does not match the level's {}",
            route_epoch.unwrap_or("none"),
            level_epoch.unwrap_or("none"),
        ));
    }
    match level_id51 {
        None | Some("none") => Ok(Identity::Unproven),
        Some(level) if Some(level) == route_id51 => Ok(Identity::Proven),
        Some(level) => Err(format!(
            "id-51 hash {} does not match the level's {level}",
            route_id51.unwrap_or("none"),
        )),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_camera_route::{
        GeneratedKind, Projection, RouteHeader, RouteOrigin, RoutePose, format_pose_file,
    };

    pub(crate) fn pose_file(hfov_degrees: f32, poses: usize) -> PoseFile {
        PoseFile {
            header: RouteHeader {
                origin: RouteOrigin::Generated(GeneratedKind::FarPoint),
                params: vec![("seed".into(), "1".into())],
                projection: Projection {
                    hfov_degrees,
                    aspect: 16.0 / 9.0,
                },
                bake: "id51=4b7255ff5bdf714c walk_epoch=4 size=10 mtime=20 prl=a.prl".into(),
                cadence_hz: 60,
                spawn: None,
            },
            poses: (0..poses)
                .map(|i| RoutePose {
                    eye: Vec3::new(i as f32, 1.5, -2.0),
                    yaw: 0.25 * i as f32,
                    pitch: -0.1,
                    walk: None,
                    cut: false,
                })
                .collect(),
        }
    }

    fn write(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn refusal(path: &Path) -> String {
        let error = load_route(path).unwrap_err();
        assert!(
            error.contains(&path.display().to_string()),
            "the error names the file: {error}"
        );
        error
    }

    #[test]
    fn load_route_reads_a_written_pose_file_bit_equal() {
        let dir = tempfile::tempdir().unwrap();
        let file = pose_file(100.0, 3);
        let path = write(&dir, "ok.txt", &format_pose_file(&file));
        assert_eq!(load_route(&path).unwrap(), file);
    }

    #[test]
    fn load_route_refuses_unreadable_empty_header_only_and_malformed_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        assert!(refusal(&missing).contains("unreadable"));

        let empty = write(&dir, "empty.txt", "");
        assert!(refusal(&empty).contains("empty"));

        let full = format_pose_file(&pose_file(100.0, 2));
        let header_only: String = full
            .lines()
            .filter(|line| !line.starts_with("pose "))
            .map(|line| format!("{line}\n"))
            .collect();
        let header_only = write(&dir, "header.txt", &header_only);
        assert!(refusal(&header_only).contains("no `pose` lines"));

        let malformed = write(
            &dir,
            "malformed.txt",
            &full.replace("pose 0 1.5", "pose zero 1.5"),
        );
        assert!(refusal(&malformed).contains("malformed"));
    }

    #[test]
    fn load_route_refuses_a_fov_outside_the_band_naming_the_value() {
        let dir = tempfile::tempdir().unwrap();
        for hfov in [59.5_f32, 131.0] {
            let path = write(&dir, "fov.txt", &format_pose_file(&pose_file(hfov, 1)));
            let error = refusal(&path);
            assert!(error.contains(&format!("{hfov}°")), "{error}");
        }
        for hfov in [MIN_FOV_DEG, MAX_FOV_DEG] {
            let path = write(&dir, "edge.txt", &format_pose_file(&pose_file(hfov, 1)));
            assert!(load_route(&path).is_ok(), "{hfov}° is inside the band");
        }
    }

    #[test]
    fn match_level_compares_id51_and_walk_epoch_only() {
        let file = pose_file(100.0, 1);
        let copy = "id51=4b7255ff5bdf714c walk_epoch=4 size=99 mtime=7 prl=elsewhere/copy.prl";
        assert_eq!(match_level(&file, copy), Ok(Identity::Proven));
        let rebaked = "id51=0000000000000000 walk_epoch=4 size=10 mtime=20 prl=a.prl";
        assert!(match_level(&file, rebaked).unwrap_err().contains("id-51"));
        let new_epoch = "id51=4b7255ff5bdf714c walk_epoch=5 size=10 mtime=20 prl=a.prl";
        assert!(
            match_level(&file, new_epoch)
                .unwrap_err()
                .contains("walk epoch")
        );
        let no_hash = "id51=none walk_epoch=4 size=10 mtime=20 prl=a.prl";
        assert_eq!(match_level(&file, no_hash), Ok(Identity::Unproven));
    }
}
