//! The external project `sdk-check` builds, made the way the bundle's own docs
//! tell a game developer to make one.
//!
//! Each step names the doc section it follows, so a doc change that breaks a
//! step points at the paragraph to re-read. The one step no doc covers is the
//! fixture level: the starter declares none, and `dist` refuses a mod that
//! ships no level.
//!
//! See: docs/external-projects.md · docs/distribution.md

use std::fs;
use std::path::Path;

use super::copy_tree;

/// The marker `docs/external-projects.md` §Laying out the project shows,
/// verbatim.
pub(super) const MARKER: &str = "[package]\nname = \"my-game\"\nmod = \"base\"\n";
pub(super) const PACKAGE: &str = "my-game";
pub(super) const MOD: &str = "base";

/// The fixture level's stem: `maps/<stem>.map` in, `maps/<stem>.prl` shipped.
pub(super) const LEVEL: &str = "sdk-check";

const FIXTURE_MAP: &str = include_str!("../../fixtures/sdk-check/sdk-check.map");
const FIXTURE_TEXTURE: &[u8] = include_bytes!("../../fixtures/sdk-check/sdk_check_grey.png");
/// Where the map's faces look for their one texture, under the mod root.
const FIXTURE_TEXTURE_PATH: &str = "textures/sdk-check/sdk_check_grey.png";

/// The starter template's own path to the SDK types, which resolves only
/// inside an install.
const TEMPLATE_TYPES_PATH: &str = "../../sdk/types/postretro.d.ts";

/// Create `project_root` as a game project against the install at `bundle`.
pub(super) fn create(bundle: &Path, project_root: &Path) -> Result<(), String> {
    let mod_root = project_root.join("content").join(MOD);

    // §Laying out the project: the marker.
    fs::create_dir_all(project_root)
        .map_err(|error| format!("create project {}: {error}", project_root.display()))?;
    write(&project_root.join("postretro.toml"), MARKER.as_bytes())?;

    // §Starting the mod: copy the starter template to `content/base/`.
    let starter = bundle.join("sdk/templates/starter");
    if !starter.is_dir() {
        return Err(format!(
            "docs/external-projects.md §Starting the mod: the bundle has no {}",
            starter.display()
        ));
    }
    copy_tree(&starter, &mod_root)?;

    // §Editor setup for TypeScript: copy the tsconfig, then point its paths at
    // the install's copy of the types.
    let tsconfig = bundle.join("sdk/templates/tsconfig.json");
    let types = bundle.join("sdk/types/postretro.d.ts");
    if !types.is_file() {
        return Err(format!(
            "docs/external-projects.md §Editor setup: the bundle has no {}",
            types.display()
        ));
    }
    let template = fs::read_to_string(&tsconfig)
        .map_err(|error| format!("read {}: {error}", tsconfig.display()))?;
    let types = types.to_str().ok_or("the bundle's path is not UTF-8")?;
    write(
        &mod_root.join("tsconfig.json"),
        point_tsconfig_at(&template, types)?.as_bytes(),
    )?;

    // The fixture level: a source map and its one texture, plus the catalog
    // entry that makes `dist` ship it (docs/distribution.md §Choose the levels
    // and bake recipes).
    write(
        &mod_root.join("maps").join(format!("{LEVEL}.map")),
        FIXTURE_MAP.as_bytes(),
    )?;
    write(&mod_root.join(FIXTURE_TEXTURE_PATH), FIXTURE_TEXTURE)?;
    let entry = mod_root.join("start-script.ts");
    let script = fs::read_to_string(&entry)
        .map_err(|error| format!("read the starter's {}: {error}", entry.display()))?;
    write(&entry, add_catalog_entry(&script)?.as_bytes())?;

    Ok(())
}

/// Replace every template path to the SDK types with the install's own, as
/// §Editor setup asks. Refuses a template that no longer names the path the
/// doc tells the reader to change.
fn point_tsconfig_at(template: &str, types: &str) -> Result<String, String> {
    if !template.contains(TEMPLATE_TYPES_PATH) {
        return Err(format!(
            "docs/external-projects.md §Editor setup: the tsconfig template no longer names \
             {TEMPLATE_TYPES_PATH}"
        ));
    }
    // JSON string escaping: a Windows path's backslashes.
    let escaped = types.replace('\\', "\\\\");
    Ok(template.replace(TEMPLATE_TYPES_PATH, &escaped))
}

/// Add a one-level `maps` catalog to the starter's `defineMod({ ... })`.
///
/// The catalog `path` is a `maps/<name>.prl` literal because the shipped level
/// set is exactly those literals in the emitted script (`build_pipeline.md`
/// §Shipped level set).
fn add_catalog_entry(script: &str) -> Result<String, String> {
    if script.contains("maps:") {
        return Err("the starter already declares a maps catalog; sdk-check expects none".into());
    }
    let close = script
        .rfind("});")
        .ok_or("the starter's entry script has no closing `});` for defineMod")?;
    let catalog = format!(
        "  maps: [\n    {{ id: \"{LEVEL}\", path: \"maps/{LEVEL}.prl\", name: \"SDK check\" }},\n  ],\n"
    );
    Ok(format!("{}{catalog}{}", &script[..close], &script[close..]))
}

fn write(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_root;

    fn read(relative: &str) -> String {
        let root = workspace_root().expect("workspace root");
        fs::read_to_string(root.join(relative)).expect(relative)
    }

    /// Drift guard against the real template: the catalog lands inside
    /// `defineMod({ ... })` as the `maps/<name>.prl` literal `dist` scans for.
    #[test]
    fn the_catalog_lands_inside_the_starters_define_mod() {
        let edited = add_catalog_entry(&read("sdk/templates/starter/start-script.ts"))
            .expect("the starter takes a catalog");
        let define = edited.find("defineMod({").expect("defineMod call");
        let literal = edited
            .find("\"maps/sdk-check.prl\"")
            .expect("catalog literal");
        let close = edited.rfind("});").expect("closing call");
        assert!(define < literal && literal < close, "{edited}");
    }

    #[test]
    fn a_starter_that_already_declares_maps_is_refused() {
        let script = "export default defineMod({\n  maps: [],\n});\n";
        assert!(add_catalog_entry(script).is_err());
    }

    /// Drift guard against the real template: every `paths` entry moves to
    /// the install's copy of the types.
    #[test]
    fn the_tsconfig_points_at_the_install_types() {
        let template = read("sdk/templates/tsconfig.json");
        let pointed = point_tsconfig_at(&template, "/opt/postretro-sdk/sdk/types/postretro.d.ts")
            .expect("the template names the types path");
        assert!(!pointed.contains(TEMPLATE_TYPES_PATH), "{pointed}");
        assert_eq!(
            pointed
                .matches("/opt/postretro-sdk/sdk/types/postretro.d.ts")
                .count(),
            template.matches(TEMPLATE_TYPES_PATH).count()
        );
    }

    /// The marker is the doc's own example, so the doc and the check cannot
    /// drift apart silently.
    #[test]
    fn the_marker_is_the_one_the_external_projects_doc_shows() {
        assert!(
            read("docs/external-projects.md").contains(&format!("```toml\n{MARKER}```")),
            "docs/external-projects.md no longer shows this marker"
        );
    }
}
