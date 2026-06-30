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
    let workspace_root = workspace_root()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    run_checked(
        Command::new(&cargo)
            .current_dir(&workspace_root)
            .arg("build")
            .arg("-p")
            .arg("postretro-script-compiler")
            .arg("--bin")
            .arg("scripts-build"),
        "build scripts-build",
    )?;

    let mut command = Command::new(&cargo);
    command
        .current_dir(&workspace_root)
        .arg("run")
        .arg("-p")
        .arg("postretro")
        .arg("--bin")
        .arg("postretro")
        .arg("--")
        .args(engine_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    status_code(
        command
            .status()
            .map_err(|e| format!("launch postretro: {e}")),
    )
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
           cargo run -p xtask -- run [postretro args...]\n\n\
         COMMANDS:\n\
           run    Build scripts-build, then run the postretro engine"
    );
}
