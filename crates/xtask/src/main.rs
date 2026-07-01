// Development workflow entry points for Postretro.
// See: context/lib/development_guide.md

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

    let mut sidecar_build = Command::new(&cargo);
    sidecar_build
        .current_dir(&workspace_root)
        .arg("build")
        .arg("-p")
        .arg("postretro-script-compiler")
        .arg("--bin")
        .arg("scripts-build")
        .args(sidecar_cargo_args);

    run_checked(&mut sidecar_build, "build scripts-build")?;

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
           cargo run -p xtask -- run [postretro args...]\n\n\
         COMMANDS:\n\
           run    Build scripts-build, then run the postretro engine\n\n\
         EXAMPLES:\n\
           cargo run -p xtask -- run content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl\n\
           cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl\n\n\
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
}
