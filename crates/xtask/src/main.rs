// Development workflow entry points for Postretro.
// See: context/lib/development_guide.md

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use postretro_level_format::prm::cache_filename_for_key;

mod crate_graph;

fn main() {
    let code = match try_main() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("xtask: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn try_main() -> Result<i32, String> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(command) = args.next() else {
        print_help();
        return Ok(1);
    };

    if command == "help" || command == "--help" || command == "-h" {
        print_help();
        return Ok(0);
    }

    if command == "run" {
        return run_postretro(args.collect());
    }

    if command == "observe" {
        return observe_headless(args.collect());
    }

    if command == "bake-model-textures" {
        return bake_model_textures_command(args.collect());
    }

    if command == "crate-graph" {
        return crate_graph::run(args.collect());
    }

    Err(format!(
        "unknown command `{}`\n\nRun `cargo run -p xtask -- --help` for usage.",
        command.to_string_lossy()
    ))
}

fn bake_model_textures_command(args: Vec<OsString>) -> Result<i32, String> {
    let gltf_path = parse_bake_model_textures_args(args)?;
    let workspace_root = workspace_root()?;
    let gltf_path = workspace_relative_path(&gltf_path, &workspace_root);
    let prm_root = workspace_root.join("baked").join("materials");
    let baked = bake_model_textures_for_gltf(&gltf_path, &prm_root)?;

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

fn parse_bake_model_textures_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [gltf_path] => Ok(PathBuf::from(gltf_path)),
        [] => Err("bake-model-textures requires a glTF path\n\n\
             Usage: cargo run -p xtask -- bake-model-textures <scene.gltf>"
            .to_string()),
        _ => Err("bake-model-textures accepts exactly one glTF path\n\n\
             Usage: cargo run -p xtask -- bake-model-textures <scene.gltf>"
            .to_string()),
    }
}

fn workspace_relative_path(path: &Path, workspace_root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BakedModelTexture {
    source_path: PathBuf,
    key_hex: String,
    prm_path: PathBuf,
}

fn bake_model_textures_for_gltf(
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

fn run_postretro(engine_args: Vec<OsString>) -> Result<i32, String> {
    let run_args = split_run_args(engine_args);
    let sidecar_cargo_args = sidecar_cargo_args(&run_args.cargo_run_args);
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    build_scripts_sidecar(&cargo, &workspace_root, &sidecar_cargo_args)?;

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .args(run_args.cargo_run_args)
        .arg("--")
        .args(run_args.engine_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
}

/// Build the `scripts-build` sidecar shared by every launch path (`run`,
/// `observe`). `sidecar_cargo_args` mirrors the subset of cargo flags that must
/// reach the sidecar build (empty for `observe`, which takes no passthrough).
fn build_scripts_sidecar(
    cargo: &OsStr,
    workspace_root: &Path,
    sidecar_cargo_args: &[OsString],
) -> Result<(), String> {
    let mut sidecar_build = Command::new(cargo);
    sidecar_build
        .current_dir(workspace_root)
        .arg("build")
        .arg("-p")
        .arg("postretro-script-compiler")
        .arg("--bin")
        .arg("scripts-build")
        .args(sidecar_cargo_args);

    run_checked(&mut sidecar_build, "build scripts-build")
}

/// `observe <runspec.json>`: build the scripts sidecar, then run the engine
/// headless under the `observability` feature. xtask is a transparent pipe — it
/// forwards the child's stdout, stderr, and exit code untouched and never parses
/// the runspec or the JSON document the engine emits. Unlike `run`, `observe`
/// takes no cargo/engine passthrough: it always builds with `--features
/// observability` and always passes `--headless`.
fn observe_headless(args: Vec<OsString>) -> Result<i32, String> {
    let runspec = parse_observe_args(args)?;
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    build_scripts_sidecar(&cargo, &workspace_root, &[])?;

    // Cargo's own build/status output goes to stderr; the engine writes the JSON
    // document to stdout. Inheriting all three streams keeps stdout pristine JSON
    // and propagates the child's exit code. Path resolution is left to cargo's
    // working directory (the workspace root), mirroring the `run` plumbing.
    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--features")
        .arg("observability")
        .arg("--")
        .arg("--headless")
        .arg(&runspec)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
}

fn parse_observe_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [runspec] => Ok(PathBuf::from(runspec)),
        [] => Err("observe requires a runspec path\n\n\
             Usage: cargo run -p xtask -- observe <runspec.json>"
            .to_string()),
        _ => Err("observe accepts exactly one runspec path\n\n\
             Usage: cargo run -p xtask -- observe <runspec.json>"
            .to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RunArgs {
    cargo_run_args: Vec<OsString>,
    engine_args: Vec<OsString>,
}

fn split_run_args(args: Vec<OsString>) -> RunArgs {
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        return RunArgs {
            cargo_run_args: Vec::new(),
            engine_args: args,
        };
    };

    RunArgs {
        cargo_run_args: args[..separator].to_vec(),
        engine_args: args[separator + 1..].to_vec(),
    }
}

fn sidecar_cargo_args(cargo_run_args: &[OsString]) -> Vec<OsString> {
    let mut sidecar_args = Vec::new();
    let mut index = 0;
    while index < cargo_run_args.len() {
        let arg = &cargo_run_args[index];
        if arg == "--release" || arg == "-r" {
            sidecar_args.push(arg.clone());
            index += 1;
            continue;
        }

        if arg == "--profile" || arg == "--target-dir" {
            sidecar_args.push(arg.clone());
            if let Some(value) = cargo_run_args.get(index + 1) {
                sidecar_args.push(value.clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if arg == "--target" {
            sidecar_args.push(arg.clone());
            if let Some(value) = cargo_run_args.get(index + 1)
                && !value.as_encoded_bytes().starts_with(b"-")
            {
                sidecar_args.push(value.clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if let Some(arg) = arg.to_str() {
            if arg.starts_with("--profile=")
                || arg.starts_with("--target=")
                || arg.starts_with("--target-dir=")
            {
                sidecar_args.push(cargo_run_args[index].clone());
            }
        }
        index += 1;
    }

    sidecar_args
}

fn run_checked(command: &mut Command, label: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{label}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label}: exited with {status}"))
    }
}

fn status_code(status: Result<std::process::ExitStatus, String>) -> Result<i32, String> {
    let status = status?;
    Ok(status.code().unwrap_or(1))
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .map(PathBuf::from)
        .ok_or_else(|| {
            format!(
                "could not derive workspace root from {}",
                manifest_dir.display()
            )
        })
}

fn print_help() {
    eprintln!(
        "Postretro development tasks\n\n\
         USAGE:\n\
           cargo run -p xtask -- run [cargo-run flags...] -- [postretro args...]\n\
           cargo run -p xtask -- run [postretro args...]\n\
           cargo run -p xtask -- observe <runspec.json>\n\
           cargo run -p xtask -- bake-model-textures <scene.gltf>\n\
           cargo run -p xtask -- crate-graph [--write | --check | --rdeps <crate> | --deps <crate>]\n\n\
         COMMANDS:\n\
           run                  Build scripts-build, then run the postretro engine\n\
           observe              Build scripts-build, then run the engine headless\n\
                                (--features observability --headless), forwarding\n\
                                the JSON document on stdout untouched\n\
           bake-model-textures  Bake glTF base-color sidecars into baked/materials\n\
           crate-graph          Analyze the internal crate dependency graph: print it,\n\
                                --write the committed snapshot, --check its freshness,\n\
                                or query --dependents / --dependencies of a crate\n\n\
         EXAMPLES:\n\
           cargo run -p xtask -- run content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- observe runspec.json\n\
           cargo run -p xtask -- bake-model-textures content/dev/models/reference_enemy_kaykit_knight/scene.gltf\n\n\
         NOTES:\n\
           Cargo flags before `--` are passed to the engine cargo run. Only\n\
           --release/-r, --profile, --target, and --target-dir are also mirrored\n\
           to the scripts-build sidecar build."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn split_run_args_without_separator_keeps_backwards_compatible_engine_args() {
        assert_eq!(
            split_run_args(os_args(&[
                "content/dev/maps/campaign-test.prl",
                "--host",
                "127.0.0.1:3456",
            ])),
            RunArgs {
                cargo_run_args: Vec::new(),
                engine_args: os_args(&[
                    "content/dev/maps/campaign-test.prl",
                    "--host",
                    "127.0.0.1:3456",
                ]),
            }
        );
    }

    #[test]
    fn split_run_args_uses_first_standalone_separator() {
        assert_eq!(
            split_run_args(os_args(&[
                "--features",
                "dev-tools",
                "--",
                "content/dev/maps/campaign-test.prl",
                "--",
                "--host",
            ])),
            RunArgs {
                cargo_run_args: os_args(&["--features", "dev-tools"]),
                engine_args: os_args(&["content/dev/maps/campaign-test.prl", "--", "--host",]),
            }
        );
    }

    #[test]
    fn sidecar_cargo_args_mirrors_profile_target_and_target_dir_flags() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--release",
                "-r",
                "--profile",
                "dev",
                "--profile=release-with-debug",
                "--target=x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--target-dir=target/other",
            ])),
            os_args(&[
                "--release",
                "-r",
                "--profile",
                "dev",
                "--profile=release-with-debug",
                "--target=x86_64-unknown-linux-gnu",
                "--target-dir",
                "target/custom",
                "--target-dir=target/other",
            ])
        );
    }

    #[test]
    fn sidecar_cargo_args_does_not_mirror_engine_package_feature_flags() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--features",
                "dev-tools",
                "--no-default-features",
                "--all-features",
                "--release",
            ])),
            os_args(&["--release"])
        );
    }

    #[test]
    fn sidecar_cargo_args_does_not_consume_feature_flag_as_optional_target_value() {
        assert_eq!(
            sidecar_cargo_args(&os_args(&[
                "--target",
                "--features",
                "dev-tools",
                "--target",
                "x86_64-unknown-linux-gnu",
            ])),
            os_args(&["--target", "--target", "x86_64-unknown-linux-gnu"])
        );
    }

    #[test]
    fn parse_observe_args_accepts_exactly_one_runspec_path() {
        assert_eq!(
            parse_observe_args(os_args(&["runspec.json"])),
            Ok(PathBuf::from("runspec.json"))
        );

        assert!(parse_observe_args(Vec::new()).is_err());
        assert!(parse_observe_args(os_args(&["a.json", "b.json"])).is_err());
    }

    #[test]
    fn parse_bake_model_textures_args_accepts_exactly_one_gltf_path() {
        assert_eq!(
            parse_bake_model_textures_args(os_args(&[
                "content/dev/models/reference_enemy_kaykit_knight/scene.gltf"
            ])),
            Ok(PathBuf::from(
                "content/dev/models/reference_enemy_kaykit_knight/scene.gltf"
            ))
        );

        assert!(parse_bake_model_textures_args(Vec::new()).is_err());
        assert!(parse_bake_model_textures_args(os_args(&["one.gltf", "two.gltf"])).is_err());
    }

    #[test]
    fn workspace_relative_path_roots_relative_paths_at_workspace() {
        let workspace = Path::new("/workspace");
        assert_eq!(
            workspace_relative_path(
                Path::new("content/dev/models/reference_enemy_kaykit_knight/scene.gltf"),
                workspace
            ),
            PathBuf::from("/workspace/content/dev/models/reference_enemy_kaykit_knight/scene.gltf")
        );
        assert_eq!(
            workspace_relative_path(Path::new("/tmp/model/scene.gltf"), workspace),
            PathBuf::from("/tmp/model/scene.gltf")
        );
    }

    #[test]
    fn bake_model_textures_for_gltf_reports_baked_keys_and_paths() {
        let gltf_path = PathBuf::from("content/dev/models/fixture/scene.gltf");
        let prm_root = PathBuf::from("/workspace/baked/materials");
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
