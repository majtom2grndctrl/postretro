// Development workflow entry points for Postretro.
//
// What lives here is what needs cargo and a workspace: launching the engine
// through `cargo run`, analysing the workspace crate graph, and building the
// binaries a distribution ships. The content tooling a distribution's recipient
// needs — `dist`, `sdk-dist`, `run`, the asset bakes, the mount solver — lives in
// `postretro-tool`, which ships inside the SDK bundle and resolves nothing at
// compile time.
//
// See: context/lib/development_guide.md

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod crate_graph;
mod dist;

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

    if command == "capture" {
        return capture_frame(args.collect());
    }

    if command == "crate-graph" {
        return crate_graph::run(args.collect());
    }

    if command == "dist" {
        return dist::dist(args.collect());
    }

    if command == "sdk-dist" {
        return dist::sdk_dist(args.collect());
    }

    Err(format!(
        "unknown command `{}`\n\nRun `cargo run -p xtask -- --help` for usage.",
        command.to_string_lossy()
    ))
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
/// takes no general cargo/engine passthrough: it always builds with `--features
/// observability` and always passes `--headless`. `--pool-seed` is the one
/// supported engine option because headless pool rolls must be pinnable.
fn observe_headless(args: Vec<OsString>) -> Result<i32, String> {
    let observe_args = parse_observe_args(args)?;
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
        .args(observe_postretro_args(&observe_args))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct ObserveArgs {
    runspec: PathBuf,
    pool_seed: Option<OsString>,
}

fn parse_observe_args(args: Vec<OsString>) -> Result<ObserveArgs, String> {
    let mut runspec = None;
    let mut pool_seed = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--pool-seed" {
            if pool_seed.is_some() {
                return Err(observe_usage("observe accepts only one --pool-seed"));
            }
            let Some(value) = args.get(index + 1) else {
                return Err(observe_usage("observe --pool-seed requires a value"));
            };
            pool_seed = Some(value.clone());
            index += 2;
            continue;
        }

        if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix("--pool-seed="))
        {
            if pool_seed.is_some() {
                return Err(observe_usage("observe accepts only one --pool-seed"));
            }
            pool_seed = Some(OsString::from(value));
            index += 1;
            continue;
        }

        if runspec.replace(PathBuf::from(arg)).is_some() {
            return Err(observe_usage("observe accepts exactly one runspec path"));
        }
        index += 1;
    }

    let runspec = runspec.ok_or_else(|| observe_usage("observe requires a runspec path"))?;
    Ok(ObserveArgs { runspec, pool_seed })
}

fn observe_postretro_args(args: &ObserveArgs) -> Vec<OsString> {
    let mut forwarded = vec![
        OsString::from("--headless"),
        args.runspec.clone().into_os_string(),
    ];
    if let Some(seed) = &args.pool_seed {
        forwarded.push(OsString::from("--pool-seed"));
        forwarded.push(seed.clone());
    }
    forwarded
}

fn observe_usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: cargo run -p xtask -- observe <runspec.json> \
         [--pool-seed <u64> | --pool-seed=<u64>]"
    )
}

/// `capture <scene.json>`: run the engine's world-only frame capture mode.
/// Unlike `observe`, capture executes no scripts and therefore needs no
/// scripts-build sidecar.
fn capture_frame(args: Vec<OsString>) -> Result<i32, String> {
    let scene = parse_capture_args(args)?;
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--features")
        .arg("capture")
        .arg("--")
        .arg("--capture")
        .arg(&scene)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro capture: {e}")),
    )
}

fn parse_capture_args(args: Vec<OsString>) -> Result<PathBuf, String> {
    match args.as_slice() {
        [scene] => Ok(PathBuf::from(scene)),
        [] => Err("capture requires a scene path\n\n\
             Usage: cargo run -p xtask -- capture <scene.json>"
            .to_string()),
        _ => Err("capture accepts exactly one scene path\n\n\
             Usage: cargo run -p xtask -- capture <scene.json>"
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
           cargo run -p xtask -- observe <runspec.json> [--pool-seed=<u64>]\n\
           cargo run -p xtask -- capture <scene.json>\n\
           cargo run -p xtask -- crate-graph [--write | --check | --mermaid | --rdeps <crate> | --deps <crate>]\n\
           cargo run -p xtask -- dist [--manifest <path>] [--out <dir>]\n\
           cargo run -p xtask -- sdk-dist [--manifest <path>] [--out <dir>]\n\n\
         COMMANDS:\n\
           run                  Build scripts-build, then run the postretro engine\n\
           observe              Build scripts-build, then run the engine headless\n\
                                (--features observability --headless), forwarding\n\
                                the JSON document on stdout untouched\n\
           capture              Run the engine's world-only frame capture\n\
                                (--features capture --capture <scene.json>)\n\
           crate-graph          Analyze the internal crate dependency graph: print it,\n\
                                --write the committed snapshot, --check its freshness,\n\
                                --mermaid the diagram, or query --rdeps / --deps of a crate\n\
           dist                 Build the release binaries, then assemble a host-native\n\
                                standalone distribution payload through postretro-tool\n\
           sdk-dist             Build the authoring and release binaries, then assemble a\n\
                                host-native content-complete modder SDK bundle through\n\
                                postretro-tool: baked maps + materials (playable out of the\n\
                                box) plus the authoring engine (debug + dev-tools), the\n\
                                compilers, postretro-tool itself, sdk/docs/tools, and the\n\
                                mod tree with sources (edit and reload)\n\n\
         CONTENT TOOLING:\n\
           Authoring runs, asset bakes, and the weapon-mount solver live in\n\
           `postretro-tool`, which needs no cargo and ships inside the SDK bundle:\n\
             cargo run -p postretro-tool -- run --install-root . content/dev/maps/campaign-test.prl\n\
             cargo run -p postretro-tool -- bake-model-textures <scene.gltf>\n\
             cargo run -p postretro-tool -- solve-weapon-mount <skeleton.gltf> ...\n\
             cargo run -p postretro-tool -- mint-identity content/dev\n\
           Run `cargo run -p postretro-tool -- --help` for its own usage. Two notes:\n\
           it runs helper binaries rather than building them, so build what it needs\n\
           first (for example `cargo build -p postretro-sim --bin mint-identity`); and\n\
           its engine-owned trees (core/, sdk/, docs/, tools/) resolve under the install\n\
           root, which a checkout build in target/ cannot derive — `dist` and `sdk-dist`\n\
           above pass `--install-root` for you, so a direct `dist`, `sdk-dist`, or\n\
           `run` invocation needs `--install-root .` from the workspace root.\n\n\
         EXAMPLES:\n\
           cargo run -p xtask -- run content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- observe runspec.json --pool-seed=17\n\n\
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
    fn parse_observe_args_accepts_runspec_without_seed() {
        assert_eq!(
            parse_observe_args(os_args(&["runspec.json"])),
            Ok(ObserveArgs {
                runspec: PathBuf::from("runspec.json"),
                pool_seed: None,
            })
        );

        assert!(parse_observe_args(Vec::new()).is_err());
        assert!(parse_observe_args(os_args(&["a.json", "b.json"])).is_err());
    }

    #[test]
    fn parse_observe_args_accepts_split_and_equals_pool_seed_and_forwards_it() {
        for input in [
            os_args(&["runspec.json", "--pool-seed", "17"]),
            os_args(&["runspec.json", "--pool-seed=17"]),
        ] {
            let parsed = parse_observe_args(input).expect("observe arguments should parse");
            assert_eq!(
                parsed,
                ObserveArgs {
                    runspec: PathBuf::from("runspec.json"),
                    pool_seed: Some(OsString::from("17")),
                }
            );
            assert_eq!(
                observe_postretro_args(&parsed),
                os_args(&["--headless", "runspec.json", "--pool-seed", "17"]),
            );
        }
    }

    #[test]
    fn parse_observe_args_rejects_missing_or_duplicate_pool_seed() {
        assert!(parse_observe_args(os_args(&["runspec.json", "--pool-seed"])).is_err());
        assert!(
            parse_observe_args(os_args(&[
                "runspec.json",
                "--pool-seed=17",
                "--pool-seed",
                "18",
            ]))
            .is_err()
        );
    }

    #[test]
    fn parse_capture_args_accepts_exactly_one_scene_path() {
        assert_eq!(
            parse_capture_args(os_args(&["scene.json"])),
            Ok(PathBuf::from("scene.json"))
        );

        assert!(parse_capture_args(Vec::new()).is_err());
        assert!(parse_capture_args(os_args(&["a.json", "b.json"])).is_err());
    }
}
