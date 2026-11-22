//! Guards the `tsconfig.json` a mod's TypeScript is edited against.
//!
//! `scripts-build` never reads a `tsconfig.json`; editors and `tsc` do. A
//! mod-root config that covers `start-script.ts` and maps every SDK module id
//! to the generated declarations is what gives `defineMod({ ... })` its
//! completions, hover docs and value errors. Without it, an editor opens the
//! start script in an inferred project where `"postretro"` does not resolve.

use super::BARE_SDK_SPECIFIERS;
use std::fs;
use std::path::{Path, PathBuf};

/// Declarations every SDK module id resolves to, relative to a mod root
/// (`content/<mod>/`) inside the engine checkout or an SDK bundle.
const DECLARATIONS_FROM_MOD_ROOT: &str = "../../sdk/types/postretro.d.ts";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn dev_mod_root_tsconfig_is_the_sdk_template() {
    let root = workspace_root();
    let template = read(&root.join("sdk/templates/tsconfig.json"));
    let dev = read(&root.join("content/dev/tsconfig.json"));
    assert_eq!(
        dev, template,
        "content/dev/tsconfig.json must match sdk/templates/tsconfig.json byte for byte: \
         the dev mod proves the template modders copy"
    );
}

#[test]
fn template_tsconfig_covers_the_start_script_and_maps_every_sdk_module() {
    let root = workspace_root();
    let config: serde_json::Value =
        serde_json::from_str(&read(&root.join("sdk/templates/tsconfig.json")))
            .expect("sdk/templates/tsconfig.json is plain JSON");

    let include = config["include"]
        .as_array()
        .expect("template tsconfig declares `include`");
    assert!(
        include.iter().any(|pattern| pattern == "**/*.ts"),
        "template must include `**/*.ts` so a mod-root copy covers start-script.ts \
         and every script below it; got {include:?}"
    );

    let paths = &config["compilerOptions"]["paths"];
    for id in BARE_SDK_SPECIFIERS {
        assert_eq!(
            paths[id],
            serde_json::json!([DECLARATIONS_FROM_MOD_ROOT]),
            "template must map SDK module `{id}` to the generated declarations"
        );
    }
    let declarations = root.join("content/dev").join(DECLARATIONS_FROM_MOD_ROOT);
    assert!(
        declarations.is_file(),
        "{} must resolve from a mod root",
        declarations.display()
    );
}
