// Postretro's shippable content tool.
//
// It ships inside SDK bundles, so it resolves nothing at compile time and
// compiles nothing at run time: it finds its project by walking up for
// `postretro.toml` and drives helper binaries somebody else built.
//
// See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::OsString;

mod binaries;
mod dist;
mod engine_trees;
mod manifest;
mod mint;
mod model_textures;
mod mount;
mod project;
mod run;
mod sdk_dist;

fn main() {
    let code = match try_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("postretro-tool: {error}");
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

    let rest: Vec<OsString> = args.collect();
    match command.to_str().unwrap_or_default() {
        "dist" => dist::run(rest),
        "sdk-dist" => sdk_dist::run(rest),
        "run" => run::run(rest),
        "bake-model-textures" => model_textures::run(rest),
        "solve-weapon-mount" => mount::run(rest),
        "mint-identity" => mint::run(rest),
        _ => Err(format!(
            "unknown command `{}`\n\nRun `postretro-tool --help` for usage.",
            command.to_string_lossy()
        )),
    }
}

/// A raw literal rather than an escaped one: `\<newline>` in a Rust string also
/// eats the next line's leading whitespace, which silently flattens every
/// indented usage line a reader relies on.
const HELP: &str = r"Postretro content tool

Every command finds its project by walking up from the working directory for
`postretro.toml`, the way cargo finds `Cargo.toml`. Pass --manifest <path> to
name one instead.

USAGE:
  postretro-tool dist [--manifest <path>] [--out <dir>]
  postretro-tool sdk-dist [--manifest <path>] [--out <dir>]
  postretro-tool run [--manifest <path>] [engine args...]
  postretro-tool bake-model-textures <scene.gltf> [--manifest <path>]
  postretro-tool solve-weapon-mount --read-muzzle-offset <viewmodel.gltf>
  postretro-tool solve-weapon-mount <skeleton.gltf> --weapon <weapon.gltf> [--check] [options]
  postretro-tool mint-identity <mod-root>

COMMANDS:
  dist                 Assemble a runnable player distribution payload
  sdk-dist             Assemble a content-complete modder SDK bundle
  run                  Launch the engine against this project, with its mod root
                       and baked materials root already correct
  bake-model-textures  Bake a glTF's base-color sidecars into baked/materials
  solve-weapon-mount   Read a viewmodel muzzle offset, solve a rigid weapon mount
                       and print the Blender bake command, or --check a baked mount
  mint-identity        Mint a mod's durable state-slot identity ledger

HELPER BINARIES:
  The tool compiles nothing. Each helper defaults to a conventional path beside
  this executable and can be named explicitly:
";

fn print_help() {
    eprintln!("{HELP}{}", binaries::HELPER_USAGE);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// `postretro-tool` ships inside distributions, so no path may be resolved
    /// at compile time: `env!("CARGO_MANIFEST_DIR")` would bake this build
    /// machine's absolute path into a stranger's binary. That is precisely why
    /// `xtask` cannot be shipped (`ui.md` §5), and the rule is easier to hold by
    /// checking than by remembering.
    ///
    /// The test reads the variable cargo sets at *run* time rather than baking
    /// it in, so it obeys the rule it enforces.
    #[test]
    fn no_source_file_resolves_a_path_at_compile_time() {
        let source_root = PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR")
                .expect("cargo sets CARGO_MANIFEST_DIR for its own test runs"),
        )
        .join("src");

        let mut offenders = Vec::new();
        collect_compile_time_path_macros(&source_root, &mut offenders);
        assert!(
            offenders.is_empty(),
            "compile-time path resolution in a shippable binary: {offenders:?}"
        );
    }

    fn collect_compile_time_path_macros(directory: &Path, offenders: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("the crate's own sources are readable") {
            let path = entry.expect("directory entry reads").path();
            if path.is_dir() {
                collect_compile_time_path_macros(&path, offenders);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let contents = std::fs::read_to_string(&path).expect("a source file is valid UTF-8");
            for (number, line) in contents.lines().enumerate() {
                let code = line.trim_start();
                // Prose about the rule is how the rule gets remembered; only
                // code can break it.
                if code.starts_with("//") {
                    continue;
                }
                if code.contains("env!(\"CARGO_MANIFEST_DIR\")")
                    || code.contains("env!(\"OUT_DIR\")")
                {
                    offenders.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
    }
}
