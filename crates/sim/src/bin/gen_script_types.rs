// Standalone generator for `postretro.d.ts` / `postretro.d.luau`.
// See: context/lib/scripting.md

#![deny(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use postretro_entities::ctx::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::typedef::write_type_definitions;
use postretro_sim::scripting::primitives::register_all;

const HELP: &str = "\
gen-script-types — regenerate the scripting type definitions

USAGE:
    gen-script-types [--out <DIR>]

OPTIONS:
    --out <DIR>   Directory to write postretro.d.ts and postretro.d.luau into
                  (default: the workspace's sdk/types)
    -h, --help    Print this help and exit
";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Write(PathBuf),
}

/// Every token is checked before anything is written: an unrecognised one is
/// a usage error rather than a silent regeneration of `sdk/types/`. Help wins
/// wherever it appears.
fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let args: Vec<String> = args.into_iter().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help);
    }
    let mut iter = args.into_iter();
    let mut out = None;
    while let Some(arg) = iter.next() {
        let value = if arg == "--out" {
            // A `--` token is the next flag, not a directory to write into.
            iter.next().filter(|value| !value.starts_with("--"))
        } else if let Some(value) = arg.strip_prefix("--out=") {
            Some(value.to_string())
        } else {
            return Err(format!("unexpected argument `{arg}`"));
        };
        let value = value
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "--out requires a directory".to_string())?;
        if out.replace(PathBuf::from(value)).is_some() {
            return Err("--out given more than once".to_string());
        }
    }
    Ok(Command::Write(out.unwrap_or_else(|| {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types"))
    })))
}

fn main() -> ExitCode {
    env_logger::try_init().ok();

    let out = match parse_args(std::env::args()) {
        Ok(Command::Write(out)) => out,
        Ok(Command::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("gen-script-types: {error}\nrun `gen-script-types --help` for usage");
            return ExitCode::from(2);
        }
    };

    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);

    if let Err(e) = write_type_definitions(&registry, &out) {
        eprintln!(
            "gen-script-types: failed to write to {}: {e}",
            out.display()
        );
        return ExitCode::from(1);
    }

    println!(
        "gen-script-types: wrote postretro.d.ts and postretro.d.luau to {}",
        out.display()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(
            std::iter::once("gen-script-types")
                .chain(args.iter().copied())
                .map(String::from),
        )
    }

    #[test]
    fn help_is_recognised_before_any_write() {
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert_eq!(parse(&["--out", "x", "--help"]), Ok(Command::Help));
        assert_eq!(parse(&["--hepl", "--help"]), Ok(Command::Help));
        assert_eq!(parse(&["--out", "--help"]), Ok(Command::Help));
    }

    #[test]
    fn out_accepts_both_forms_and_rejects_everything_else() {
        assert_eq!(parse(&["--out", "a"]), Ok(Command::Write("a".into())));
        assert_eq!(parse(&["--out=a"]), Ok(Command::Write("a".into())));
        assert!(matches!(parse(&[]), Ok(Command::Write(_))));
        for args in [
            &["--hepl"][..],
            &["--out"],
            &["--out", "--hepl"],
            &["--out="],
            &["--out", ""],
            &["stray"],
            &["--out=a", "--out=b"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }
}
