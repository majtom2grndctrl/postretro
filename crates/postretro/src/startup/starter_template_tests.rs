// The SDK bundle's starter mod: `sdk/templates/starter/start-script.ts`, which
// `sdk-dist` compiles and publishes at `content/base`. It is the only mod a
// bundle ships, and `postretro-tool run` from the bundle root boots it, so it
// must bundle and initialize as a mod manifest exactly as boot runs it —
// durable-identity enforcement included, since a bundle carries no ledger for
// it.
// See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::path::Path;

use postretro_entities::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};

use crate::scripting::primitives::register_all;

#[test]
fn sdk_starter_mod_bundles_and_initializes_with_no_content() {
    let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/templates/starter/start-script.ts");
    let bundle = postretro_script_compiler::bundle_entry(&entry).expect("starter bundles");

    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    let mut runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx)
        .expect("script runtime constructs");
    let mod_root = tempfile::tempdir().expect("temp mod root");
    std::fs::write(mod_root.path().join("start-script.js"), bundle).expect("start script writes");
    runtime
        .run_mod_init(mod_root.path())
        .expect("the starter initializes as a mod manifest");

    let manifest = runtime.mod_manifest().expect("mod init commits a manifest");
    assert!(
        manifest.maps.is_empty(),
        "the starter ships no content, so it offers no levels"
    );
}
