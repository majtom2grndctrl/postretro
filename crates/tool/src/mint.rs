//! `mint-identity`: mint a mod's durable state-slot identity ledger.
//!
//! The mint stays a separate binary the tool execs rather than linked code.
//! `crates/sim/src/bin/mint_identity.rs` pulls in `postretro-scripting-core`'s
//! runtime, so linking it would drag rquickjs and mlua into a tool that needs
//! neither — and would make the tool depend upward on `postretro-sim`. The mint
//! owns its own diagnostics; this forwards stdio and status.
//!
//! See: context/lib/scripting.md §5

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::binaries::{Helper, Overrides, status_code};

/// The only helper `mint-identity` drives is the mint binary itself. The mint
/// finds `scripts-build` on its own at runtime (beside itself or on `PATH`), so
/// `--scripts-build` — and every other helper flag — is not this command's to
/// accept. Recognising them here would let a reader who trusts the usage text's
/// mention of `scripts-build` pass a flag that is then silently ignored.
const MINT_HELPERS: &[Helper] = &[Helper::MintIdentity];

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let (supplied_mod_root, binaries) = parse_args(args)?;
    let invocation_dir = std::env::current_dir()
        .map_err(|error| format!("resolve mint-identity invocation directory: {error}"))?;
    let mod_root = absolute_mod_root(&supplied_mod_root, &invocation_dir);
    let mint = binaries.resolve(Helper::MintIdentity)?;

    let mut command = Command::new(&mint);
    command
        .arg(&mod_root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    status_code(command.status())
}

/// A relative mod root resolves against the working directory, so the path the
/// mint receives means the same thing the caller typed.
fn absolute_mod_root(supplied: &Path, invocation_dir: &Path) -> PathBuf {
    if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        invocation_dir.join(supplied)
    }
}

fn parse_args(args: Vec<OsString>) -> Result<(PathBuf, Overrides), String> {
    let mut mod_root = None;
    let mut binaries = Overrides::default();
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].to_str().unwrap_or_default();
        if binaries.absorb_only(argument, args.get(index + 1), MINT_HELPERS)? {
            index += 2;
            continue;
        }
        if argument.starts_with('-') {
            return Err(usage(&format!("unknown mint-identity option {argument:?}")));
        }
        if mod_root.replace(PathBuf::from(&args[index])).is_some() {
            return Err(usage("mint-identity accepts exactly one mod root"));
        }
        index += 1;
    }

    let mod_root = mod_root.ok_or_else(|| usage("mint-identity requires a mod root"))?;
    Ok((mod_root, binaries))
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool mint-identity <mod-root> [--mint-identity <path>]\n\n\
         A TypeScript mod is compiled by the mint's own runtime, which looks for \
         `scripts-build` beside the mint binary or on PATH."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_args_requires_exactly_one_mod_root() {
        let (mod_root, _) = parse_args(os_args(&["content/dev"])).expect("one path is valid");
        assert_eq!(mod_root, PathBuf::from("content/dev"));

        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(os_args(&["one", "two"])).is_err());
    }

    #[test]
    fn mint_identity_accepts_the_mint_override_it_actually_drives() {
        let (mod_root, _binaries) = parse_args(os_args(&[
            "content/dev",
            "--mint-identity",
            "/build/mint-identity",
        ]))
        .expect("the one helper this command drives is accepted");
        // The `--mint-identity` token and its path were consumed as the override,
        // not mistaken for a second mod root.
        assert_eq!(mod_root, PathBuf::from("content/dev"));
    }

    /// Pointed: the usage text mentions `scripts-build`, but the mint finds that
    /// compiler itself — this flag is not `mint-identity`'s to accept, and
    /// silently ignoring it is the defect being closed.
    #[test]
    fn mint_identity_rejects_a_helper_flag_it_never_consumes() {
        let error = parse_args(os_args(&[
            "content/dev",
            "--scripts-build",
            "/build/scripts-build",
        ]))
        .expect_err("a helper flag this command does not drive is rejected");
        assert!(error.contains("--scripts-build"), "{error}");
    }

    #[test]
    fn relative_mod_roots_resolve_against_the_invocation_directory() {
        assert_eq!(
            absolute_mod_root(Path::new("mods/ts"), Path::new("/work/project")),
            Path::new("/work/project/mods/ts")
        );
        assert_eq!(
            absolute_mod_root(Path::new("/installed/mod"), Path::new("/unrelated")),
            Path::new("/installed/mod")
        );
    }
}
