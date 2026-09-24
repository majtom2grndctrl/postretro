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
use std::process::{Command, Stdio};

use crate::binaries::{Helper, Overrides, status_code};
use crate::manifest::{mod_root_rel, validate_mod_name};
use crate::project::ProjectLocation;

/// The only helper `mint-identity` drives is the mint binary itself. The mint
/// finds `scripts-build` on its own at runtime (beside itself or on `PATH`), so
/// `--scripts-build` — and every other helper flag — is not this command's to
/// accept. Recognising them here would let a reader who trusts the usage text's
/// mention of `scripts-build` pass a flag that is then silently ignored.
const MINT_HELPERS: &[Helper] = &[Helper::MintIdentity];

/// The command's own arguments: which project, which mod in it, and the mint.
struct MintArgs {
    location: ProjectLocation,
    mod_name: String,
    binaries: Overrides,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let mut cli = parse_args(args)?;
    let working_directory = std::env::current_dir()
        .map_err(|error| format!("resolve mint-identity invocation directory: {error}"))?;
    // A mod is named, not pathed, so it resolves against the project rather than
    // wherever the caller stood — `content/<mod>` under the project root, the
    // same place the engine's `--mod <mod>` looks. Rebased first so the mint
    // receives an absolute path whichever way the project was named.
    cli.location.rebase(&working_directory);
    let project = cli.location.open(&working_directory)?;
    let mod_root = project.join(&mod_root_rel(&cli.mod_name));
    let mint = cli.binaries.resolve(Helper::MintIdentity)?;

    let mut command = Command::new(&mint);
    command
        .arg(&mod_root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    status_code(command.status())
}

fn parse_args(args: Vec<OsString>) -> Result<MintArgs, String> {
    let mut location = ProjectLocation::default();
    let mut mod_name: Option<String> = None;
    let mut binaries = Overrides::default();
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].to_str().unwrap_or_default();
        let value = args.get(index + 1);
        let consumed = binaries.absorb_only(argument, value, MINT_HELPERS)?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        let consumed = location
            .absorb(argument, value)
            .map_err(|error| usage(&error))?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        if argument.starts_with('-') {
            return Err(usage(&format!("unknown mint-identity option {argument:?}")));
        }
        validate_mod_name(argument).map_err(|error| usage(&error))?;
        if mod_name.replace(argument.to_string()).is_some() {
            return Err(usage("mint-identity accepts exactly one mod"));
        }
        index += 1;
    }

    let mod_name = mod_name.ok_or_else(|| usage("mint-identity requires a mod name"))?;
    Ok(MintArgs {
        location,
        mod_name,
        binaries,
    })
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool mint-identity <mod> [--project <dir> | --manifest <path>] \
         [--mint-identity <path>]\n\n\
         <mod> is the mod's name — the directory under the project's `content/`.\n\
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
    fn parse_args_requires_exactly_one_mod_name() {
        let parsed = parse_args(os_args(&["dev"])).expect("one mod name is valid");
        assert_eq!(parsed.mod_name, "dev");

        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(os_args(&["one", "two"])).is_err());
    }

    /// The mod is named, not pathed — including the old `content/<mod>`
    /// spelling, which would otherwise resolve `content/content/<mod>`.
    #[test]
    fn mint_identity_refuses_a_path_where_the_mod_name_belongs() {
        for path in ["content/dev", "/installed/mod", "..", "C:dev"] {
            let error = parse_args(os_args(&[path]))
                .err()
                .expect("a path is not a mod name");
            assert!(error.contains("must be a mod name"), "{path}: {error}");
        }
    }

    #[test]
    fn mint_identity_accepts_the_mint_override_it_actually_drives() {
        let parsed = parse_args(os_args(&["dev", "--mint-identity", "/build/mint-identity"]))
            .expect("the one helper this command drives is accepted");
        // The `--mint-identity` token and its path were consumed as the override,
        // not mistaken for a second mod.
        assert_eq!(parsed.mod_name, "dev");
    }

    #[test]
    fn mint_identity_takes_the_project_flags() {
        let parsed = parse_args(os_args(&["dev", "--project", "/projects/game"]))
            .expect("the project may be named");
        assert_eq!(parsed.mod_name, "dev");
        assert_eq!(
            parsed.location.directory(),
            Some(std::path::Path::new("/projects/game"))
        );
    }

    /// The project and helper-binary flags accept the equals form here too.
    #[test]
    fn mint_identity_takes_the_equals_form_of_its_flags() {
        let parsed = parse_args(os_args(&[
            "dev",
            "--project=/projects/game",
            "--mint-identity=/build/mint-identity",
        ]))
        .expect("the equals form of both flags parses");
        assert_eq!(parsed.mod_name, "dev");
        assert_eq!(
            parsed.location.directory(),
            Some(std::path::Path::new("/projects/game"))
        );
    }

    /// Pointed: the usage text mentions `scripts-build`, but the mint finds that
    /// compiler itself — this flag is not `mint-identity`'s to accept, and
    /// silently ignoring it is the defect being closed.
    #[test]
    fn mint_identity_rejects_a_helper_flag_it_never_consumes() {
        let error = parse_args(os_args(&["dev", "--scripts-build", "/build/scripts-build"]))
            .err()
            .expect("a helper flag this command does not drive is rejected");
        assert!(error.contains("--scripts-build"), "{error}");
    }
}
