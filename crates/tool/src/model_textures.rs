//! `bake-model-textures`: the model base-color `.prm` bake, standalone.
//!
//! `prl-build` bakes model sidecars only for `prop_mesh` placements, so a rig,
//! viewmodel, or enemy declared in script reaches a payload with no sidecar and
//! renders placeholders without failing. This runs the same bake against one
//! glTF, and the distribution's stage 4 runs it across the mod's `models/` tree.
//!
//! See: context/lib/build_pipeline.md §Baked texture mips (Model textures)

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use postretro_level_format::prm::cache_filename_for_key;

use crate::project::ProjectLocation;

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let (gltf_path, location) = parse_args(args)?;
    let working_directory =
        std::env::current_dir().map_err(|error| format!("read the working directory: {error}"))?;
    let project = location.open(&working_directory)?;

    let baked = bake_model_textures_for_gltf(&gltf_path, &project.materials_root())?;
    if baked.is_empty() {
        println!(
            "No filesystem base-color textures found in {}",
            gltf_path.display()
        );
        return Ok(0);
    }

    for texture in baked {
        println!(
            "Baked {} -> {} (key {})",
            texture.source_path.display(),
            texture.prm_path.display(),
            texture.key_hex
        );
    }
    Ok(0)
}

/// One glTF path, resolved against the working directory the way any CLI path
/// argument is, plus however the caller named the project.
fn parse_args(args: Vec<OsString>) -> Result<(PathBuf, ProjectLocation), String> {
    let mut gltf_path = None;
    let mut location = ProjectLocation::default();
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].to_str().ok_or_else(|| {
            format!(
                "bake-model-textures argument {} is not valid UTF-8",
                args[index].to_string_lossy()
            )
        })?;
        let consumed = location
            .absorb(argument, args.get(index + 1))
            .map_err(|error| usage(&error))?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        if argument.starts_with('-') {
            return Err(usage(&format!(
                "unknown bake-model-textures option {argument:?}"
            )));
        }
        if gltf_path.replace(PathBuf::from(argument)).is_some() {
            return Err(usage("bake-model-textures accepts exactly one glTF path"));
        }
        index += 1;
    }

    let gltf_path = gltf_path.ok_or_else(|| usage("bake-model-textures requires a glTF path"))?;
    Ok((gltf_path, location))
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool bake-model-textures <scene.gltf> \
         [--project <dir> | --manifest <path>]"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BakedModelTexture {
    pub(crate) source_path: PathBuf,
    pub(crate) key_hex: String,
    pub(crate) prm_path: PathBuf,
}

pub(crate) fn bake_model_textures_for_gltf(
    gltf_path: &Path,
    prm_root: &Path,
) -> Result<Vec<BakedModelTexture>, String> {
    bake_model_textures_for_gltf_with(
        gltf_path,
        prm_root,
        postretro_level_format::gltf_resolve::resolve_document_base_color_paths,
        postretro_level_compiler::texture_mips::bake_diffuse_texture,
    )
}

fn bake_model_textures_for_gltf_with<Resolve, ResolveError, Bake, BakeError>(
    gltf_path: &Path,
    prm_root: &Path,
    mut resolve_base_color_paths: Resolve,
    mut bake_diffuse: Bake,
) -> Result<Vec<BakedModelTexture>, String>
where
    Resolve: FnMut(&Path) -> Result<Vec<PathBuf>, ResolveError>,
    ResolveError: Display,
    Bake: FnMut(&Path, &Path) -> Result<[u8; 32], BakeError>,
    BakeError: Display,
{
    let texture_paths = resolve_base_color_paths(gltf_path).map_err(|error| {
        format!(
            "resolve model textures for {}: {error}",
            gltf_path.display()
        )
    })?;

    let mut seen = HashSet::new();
    let mut baked = Vec::new();
    for texture_path in texture_paths {
        if !seen.insert(texture_path.clone()) {
            continue;
        }

        let key = bake_diffuse(&texture_path, prm_root)
            .map_err(|error| format!("bake model texture {}: {error}", texture_path.display()))?;
        let key_hex = cache_filename_for_key(&key);
        baked.push(BakedModelTexture {
            source_path: texture_path,
            prm_path: prm_root.join(format!("{key_hex}.prm")),
            key_hex,
        });
    }

    Ok(baked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_args_accepts_one_gltf_path_with_an_optional_project() {
        let (gltf, location) =
            parse_args(os_args(&["models/knight/scene.gltf"])).expect("a bare path is valid");
        assert_eq!(gltf, PathBuf::from("models/knight/scene.gltf"));
        assert_eq!(location, ProjectLocation::default());

        let (gltf, location) = parse_args(os_args(&[
            "scene.gltf",
            "--manifest",
            "/projects/game/postretro.toml",
        ]))
        .expect("a named marker is valid");
        assert_eq!(gltf, PathBuf::from("scene.gltf"));
        assert_eq!(
            location.manifest(),
            Some(Path::new("/projects/game/postretro.toml"))
        );

        let (_, location) = parse_args(os_args(&["scene.gltf", "--project", "/projects/game"]))
            .expect("a named project directory is valid");
        assert_eq!(location.directory(), Some(Path::new("/projects/game")));

        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(os_args(&["one.gltf", "two.gltf"])).is_err());
        assert!(parse_args(os_args(&["scene.gltf", "--manifest"])).is_err());
    }

    /// The equals form is accepted here too, and does not get mistaken for a
    /// second glTF path.
    #[test]
    fn parse_args_accepts_the_equals_form_of_project_flags() {
        let (gltf, location) = parse_args(os_args(&["scene.gltf", "--project=/projects/game"]))
            .expect("the equals form of --project parses");
        assert_eq!(gltf, PathBuf::from("scene.gltf"));
        assert_eq!(location.directory(), Some(Path::new("/projects/game")));
    }

    #[test]
    fn bake_model_textures_for_gltf_reports_baked_keys_and_paths() {
        let gltf_path = PathBuf::from("content/dev/models/fixture/scene.gltf");
        let prm_root = PathBuf::from("/project/baked/materials");
        let diffuse_a = PathBuf::from("content/dev/models/fixture/a.png");
        let diffuse_b = PathBuf::from("content/dev/models/fixture/b.png");

        let baked = bake_model_textures_for_gltf_with(
            &gltf_path,
            &prm_root,
            |path| {
                assert_eq!(path, gltf_path.as_path());
                Ok::<_, String>(vec![
                    diffuse_a.clone(),
                    diffuse_a.clone(),
                    diffuse_b.clone(),
                ])
            },
            |path, root| {
                assert_eq!(root, prm_root.as_path());
                let mut key = [0u8; 32];
                key[31] = if path == diffuse_a.as_path() { 1 } else { 2 };
                Ok::<_, String>(key)
            },
        )
        .expect("fake bake should succeed");

        assert_eq!(
            baked,
            vec![
                BakedModelTexture {
                    source_path: diffuse_a,
                    key_hex: format!("{}01", "0".repeat(62)),
                    prm_path: prm_root.join(format!("{}01.prm", "0".repeat(62))),
                },
                BakedModelTexture {
                    source_path: diffuse_b,
                    key_hex: format!("{}02", "0".repeat(62)),
                    prm_path: prm_root.join(format!("{}02.prm", "0".repeat(62))),
                },
            ]
        );
    }
}
