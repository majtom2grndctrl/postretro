// Build script: generates the SDK QuickJS prelude into OUT_DIR. See: context/lib/scripting.md §7

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let sdk_root = Path::new(&manifest_dir)
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above crates/scripting-core/")
        .join("sdk/lib");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = Path::new(&out_dir).join("prelude.js");

    postretro_script_compiler::write_prelude(&sdk_root, &out_path)
        .unwrap_or_else(|e| panic!("failed to generate SDK prelude: {e:#}"));

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", sdk_root.display());
    emit_ts_rerun_directives(&sdk_root);
}

fn emit_ts_rerun_directives(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("failed to read sdk dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|e| panic!("failed to read entry in {}: {e}", dir.display()));
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("failed to stat {}: {e}", path.display()));
        if file_type.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            emit_ts_rerun_directives(&path);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "ts") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
