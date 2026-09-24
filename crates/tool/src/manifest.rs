//! `postretro.toml` — the project marker's schema and boundary validation.
//!
//! The same file identifies a Postretro project (`project.rs`) and describes the
//! distribution it builds, so its paths are project-relative rather than
//! workspace-relative. They intentionally remain slash-separated strings: the
//! resolver compares a recipe output directly with literals scanned from the
//! emitted entry script.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Manifest {
    pub(crate) package: Package,
    pub(crate) recipes: Vec<Recipe>,
}

/// Every mod lives directly under this project directory. The manifest names a
/// mod, never a path, and the tool puts it here — the same rule the engine's
/// `--mod <name>` applies. Two components is the shape the runtime's `baked/`
/// grandparent derivation needs (`build_pipeline.md` §Baked texture mips).
pub(crate) const CONTENT_DIR: &str = "content";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Package {
    pub(crate) name: String,
    /// The mod's name: one directory directly under [`CONTENT_DIR`].
    pub(crate) mod_name: String,
    /// `content/<mod_name>`, derived once so project-relative joins and
    /// distribution destinations share one spelling.
    pub(crate) mod_root: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recipe {
    /// The mod-root-relative `maps/<name>.prl` literal emitted by the entry script.
    pub(crate) output: String,
    /// An optional project-relative `.map` source path.
    pub(crate) source: Option<String>,
    /// Additional, individual `prl-build` arguments supplied by the manifest.
    pub(crate) args: Vec<String>,
    /// The validated effective density, retained for deterministic bake ordering.
    pub(crate) lightmap_density: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default)]
    recipes: Vec<RawRecipe>,
}

/// `mod` is optional here, and the retired `mod_root` key is read at all, only
/// so a manifest still carrying `mod_root = "content/<name>"` is told what to
/// write instead, rather than only that `mod` is missing. Other keys stay
/// ignored, as they always were.
#[derive(Debug, Deserialize)]
struct RawPackage {
    name: String,
    #[serde(rename = "mod")]
    mod_name: Option<String>,
    mod_root: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawRecipe {
    output: String,
    source: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

impl Manifest {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|error| format!("read project manifest {}: {error}", path.display()))?;
        Self::parse(&contents)
            .map_err(|error| format!("parse project manifest {}: {error}", path.display()))
    }

    pub(crate) fn parse(contents: &str) -> Result<Self, String> {
        let raw: RawManifest =
            toml::from_str(contents).map_err(|error| format!("invalid TOML: {error}"))?;
        let mod_name = validate_package(&raw.package)?;

        let mut outputs = HashSet::new();
        let mut recipes = Vec::with_capacity(raw.recipes.len());
        for raw_recipe in raw.recipes {
            let label = format!("recipe `{}`", raw_recipe.output);
            validate_recipe_path(&raw_recipe.output, "output", &label)?;
            if !raw_recipe.output.starts_with("maps/") {
                return Err(format!("{label}: output must start with `maps/`"));
            }
            if let Some(source) = &raw_recipe.source {
                validate_recipe_path(source, "source", &label)?;
            }
            if !outputs.insert(raw_recipe.output.clone()) {
                return Err(format!("{label}: duplicate output"));
            }
            let lightmap_density = validate_args(&raw_recipe.args, &label)?;
            recipes.push(Recipe {
                output: raw_recipe.output,
                source: raw_recipe.source,
                args: raw_recipe.args,
                lightmap_density,
            });
        }

        Ok(Self {
            package: Package {
                name: raw.package.name,
                mod_root: mod_root_rel(&mod_name),
                mod_name,
            },
            recipes,
        })
    }
}

/// The validated mod name.
fn validate_package(package: &RawPackage) -> Result<String, String> {
    if !is_normal_component(&package.name) {
        return Err(format!(
            "package `{}`: name must be one normal path component",
            package.name
        ));
    }
    if package.mod_root.is_some() {
        return Err(format!(
            "package: `mod_root` was replaced by `mod`, which takes the mod's name \
             rather than its path (for `mod_root = \"{CONTENT_DIR}/dev\"`, write `mod = \"dev\"`)"
        ));
    }
    let mod_name = package.mod_name.clone().ok_or_else(|| {
        format!(
            "package: missing `mod`, the mod's name — one directory directly under \
             `{CONTENT_DIR}/` (for `{CONTENT_DIR}/dev`, write `mod = \"dev\"`)"
        )
    })?;
    validate_mod_name(&mod_name).map_err(|error| format!("package {error}"))?;
    Ok(mod_name)
}

/// A mod is named, never pathed: one plain directory name, which the tool
/// places under [`CONTENT_DIR`]. A path is refused rather than joined, since
/// `content/dev` would otherwise land at `content/content/dev`.
///
/// A leading `-` is refused too, matching the engine's `--mod` check: the
/// launcher and `run` pass the name as the argument after `--mod`, where a
/// flag-shaped value is refused at boot. Refusing it here reports the mistake
/// when the manifest is read, not when a shipped launcher is first run.
pub(crate) fn validate_mod_name(name: &str) -> Result<(), String> {
    if name.starts_with('-') {
        Err(format!(
            "mod `{name}`: a mod name cannot start with `-`, since `--mod {name}` reads as a flag"
        ))
    } else if is_normal_component(name) {
        Ok(())
    } else {
        Err(format!(
            "mod `{name}`: must be a mod name — one directory directly under \
             `{CONTENT_DIR}/` — not a path (for `{CONTENT_DIR}/dev`, write `dev`)"
        ))
    }
}

/// The project-relative mod root for a validated mod name.
pub(crate) fn mod_root_rel(mod_name: &str) -> String {
    format!("{CONTENT_DIR}/{mod_name}")
}

fn validate_recipe_path(path: &str, field: &str, label: &str) -> Result<(), String> {
    if slash_components(path).is_none() {
        return Err(format!(
            "{label}: {field} must be a project-relative `/` path"
        ));
    }
    Ok(())
}

fn slash_components(path: &str) -> Option<Vec<&str>> {
    if path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let components: Vec<_> = path.split('/').collect();
    (!components.is_empty()
        && components
            .iter()
            .all(|component| is_normal_component(component)))
    .then_some(components)
}

fn is_normal_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['/', '\\', ':'])
}

/// Arguments a recipe may never supply, because the tool owns them.
///
/// `-o`, `--release`, and the TUI switches decide *what kind of bake* a
/// distribution runs, and `--release` is the only shippable one. `--baked-root`
/// and `--cache-dir` are the pair that must stay consistent across the compiler
/// and the engine: prl-build reads the last occurrence of each, so a recipe
/// naming one would silently override the tool's and reintroduce exactly the
/// placeholder degradation the flags exist to close.
const TOOL_OWNED_ARGS: [&str; 6] = [
    "-o",
    "--release",
    "--tui",
    "--no-tui",
    "--baked-root",
    "--cache-dir",
];

fn validate_args(args: &[String], label: &str) -> Result<Option<f32>, String> {
    let mut density = None;
    for (index, arg) in args.iter().enumerate() {
        let owned = TOOL_OWNED_ARGS.iter().find(|owned| {
            arg == *owned || arg.split_once('=').is_some_and(|(flag, _)| flag == **owned)
        });
        if let Some(owned) = owned {
            return Err(format!("{label}: args may not contain `{owned}`"));
        }
        if arg.starts_with("--lightmap-density=") {
            return Err(format!(
                "{label}: use `--lightmap-density` and a separate value token"
            ));
        }
        if arg != "--lightmap-density" {
            continue;
        }
        if density.is_some() {
            return Err(format!(
                "{label}: `--lightmap-density` appears more than once"
            ));
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{label}: `--lightmap-density` requires an f32 value token"))?;
        let parsed = value
            .parse::<f32>()
            .map_err(|_| format!("{label}: `--lightmap-density` value `{value}` is not an f32"))?;
        if !parsed.is_finite() || parsed <= 0.0 {
            return Err(format!(
                "{label}: `--lightmap-density` value `{value}` must be finite and greater than zero"
            ));
        }
        density = Some(parsed);
    }
    Ok(density)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV_MANIFEST: &str = r#"
[package]
name = "postretro-dev"
mod = "dev"
"#;

    #[test]
    fn parses_minimal_manifest() {
        let manifest = Manifest::parse(DEV_MANIFEST).expect("manifest parses");
        assert_eq!(manifest.package.name, "postretro-dev");
        assert_eq!(manifest.package.mod_name, "dev");
        // The mod name places the mod under `content/`; nothing spells the path.
        assert_eq!(manifest.package.mod_root, "content/dev");
        assert!(manifest.recipes.is_empty());
    }

    /// The retired key is named in the error, so an older manifest says what to
    /// change instead of only that `mod` is missing.
    #[test]
    fn rejects_the_retired_mod_root_key_by_name() {
        for input in [
            "[package]\nname = \"dev\"\nmod_root = \"content/dev\"\n",
            "[package]\nname = \"dev\"\nmod = \"dev\"\nmod_root = \"content/dev\"\n",
        ] {
            let error = Manifest::parse(input).unwrap_err();
            assert!(
                error.contains("`mod_root` was replaced by `mod`"),
                "{error}"
            );
            assert!(error.contains("mod = \"dev\""), "{error}");
        }
    }

    #[test]
    fn rejects_a_package_without_a_mod() {
        let error = Manifest::parse("[package]\nname = \"dev\"\n").unwrap_err();
        assert!(error.contains("missing `mod`"), "{error}");
    }

    /// Only the retired key is refused; unrelated `[package]` keys stay ignored,
    /// as they were before the key was renamed.
    #[test]
    fn ignores_unrelated_package_keys() {
        let manifest =
            Manifest::parse("[package]\nname = \"dev\"\nmod = \"dev\"\nversion = \"1.0\"\n")
                .expect("an unrelated key is ignored");
        assert_eq!(manifest.package.mod_name, "dev");
    }

    #[test]
    fn parses_recipe_and_density() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "dev"
mod = "dev"
[[recipes]]
output = "maps/custom.prl"
source = "content/dev/maps/source.map"
args = ["--lightmap-density", "0.02"]
"#,
        )
        .expect("manifest parses");
        assert_eq!(manifest.recipes[0].lightmap_density, Some(0.02));
    }

    #[test]
    fn rejects_every_invalid_package_name_and_mod_name() {
        for name in [".", "..", "C:", "nested/name", "nested\\\\name", ""] {
            let input = format!("[package]\nname = \"{name}\"\nmod = \"dev\"\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("package"), "{error}");
        }
        // A path where a name belongs — including the old `content/<name>`
        // spelling — is refused rather than nested under `content/`.
        for mod_name in [
            "",
            ".",
            "..",
            "content/dev",
            "dev/",
            "/dev",
            "content\\\\dev",
            "C:dev",
            "dev:stream",
        ] {
            let input = format!("[package]\nname = \"dev\"\nmod = \"{mod_name}\"\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("must be a mod name"), "{mod_name}: {error}");
        }
    }

    /// A name that looks like a flag would reach the engine as `--mod -x`, which
    /// the engine refuses at boot — so the manifest refuses it first.
    #[test]
    fn rejects_a_mod_name_that_looks_like_a_flag() {
        for mod_name in ["-x", "--dev"] {
            let input = format!("[package]\nname = \"dev\"\nmod = \"{mod_name}\"\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(
                error.contains("cannot start with `-`"),
                "{mod_name}: {error}"
            );
        }
    }

    // Regression: Windows path prefixes could replace the workspace root at join time.
    #[test]
    fn rejects_windows_prefix_and_colon_bearing_manifest_paths() {
        for (field, value) in [
            ("output", "maps/C:/a.prl"),
            ("output", "maps/a.prl:stream"),
            ("source", "C:/content/dev/maps/a.map"),
            ("source", "content/dev/maps/a.map:stream"),
        ] {
            let source = if field == "source" {
                format!("source = \"{value}\"\n")
            } else {
                String::new()
            };
            let output = if field == "output" {
                value
            } else {
                "maps/a.prl"
            };
            let input = format!("{DEV_MANIFEST}\n[[recipes]]\noutput = \"{output}\"\n{source}");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe `"), "{error}");
            assert!(error.contains(field), "{error}");
        }
    }

    #[test]
    fn rejects_duplicate_recipe_output() {
        let input = format!(
            "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\n\n[[recipes]]\noutput = \"maps/a.prl\"\n"
        );
        assert!(
            Manifest::parse(&input)
                .unwrap_err()
                .contains("recipe `maps/a.prl`")
        );
    }

    #[test]
    fn rejects_reserved_bake_arguments() {
        for arg in ["-o", "--release", "--tui", "--no-tui"] {
            let input = format!(
                "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\nargs = [\"{arg}\"]\n"
            );
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe `maps/a.prl`"), "{error}");
            assert!(error.contains(arg), "{error}");
        }
    }

    /// The tool passes the materials root and the stage cache directory itself,
    /// on both sides of the compiler/engine contract. prl-build takes the last
    /// occurrence of each, so a recipe that named one would win — and silently
    /// send the `.prm` sidecars somewhere the engine does not read.
    #[test]
    fn rejects_the_root_and_cache_flags_the_tool_owns_in_both_spellings() {
        for arg in [
            "--baked-root",
            "--baked-root=/elsewhere/baked",
            "--cache-dir",
            "--cache-dir=/elsewhere/cache",
        ] {
            let input = format!(
                "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\nargs = [\"{arg}\", \"x\"]\n"
            );
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe `maps/a.prl`"), "{error}");
            assert!(
                error.contains("--baked-root") || error.contains("--cache-dir"),
                "{error}"
            );
        }
    }

    #[test]
    fn rejects_malformed_density_arguments() {
        for args in [
            "[\"--lightmap-density=0.02\"]",
            "[\"--lightmap-density\"]",
            "[\"--lightmap-density\", \"not-a-number\"]",
            "[\"--lightmap-density\", \"0.01\", \"--lightmap-density\", \"0.02\"]",
        ] {
            let input =
                format!("{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\nargs = {args}\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe `maps/a.prl`"), "{error}");
        }
    }

    // Regression: non-positive and non-finite densities reached prl-build before failing.
    #[test]
    fn rejects_density_outside_finite_strictly_positive_range() {
        for value in ["0", "-0", "-0.01", "NaN", "inf", "-inf"] {
            let input = format!(
                "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\nargs = [\"--lightmap-density\", \"{value}\"]\n"
            );
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe `maps/a.prl`"), "{error}");
            assert!(error.contains("finite and greater than zero"), "{error}");
        }
    }

    #[test]
    fn accepts_finite_strictly_positive_density_boundaries() {
        for value in ["1e-45", "3.4028235e38"] {
            let input = format!(
                "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\nargs = [\"--lightmap-density\", \"{value}\"]\n"
            );
            let density = Manifest::parse(&input)
                .expect("finite positive boundary parses")
                .recipes[0]
                .lightmap_density
                .expect("density retained");
            assert!(density.is_finite());
            assert!(density > 0.0);
        }
    }

    #[test]
    fn rejects_invalid_recipe_paths_and_non_map_outputs() {
        for (field, value) in [
            ("output", "maps/../a.prl"),
            ("output", "maps\\\\a.prl"),
            ("source", "/content/dev/maps/a.map"),
            ("source", "content//dev/maps/a.map"),
        ] {
            let source = if field == "source" {
                format!("source = \"{value}\"\n")
            } else {
                String::new()
            };
            let output = if field == "output" {
                value
            } else {
                "maps/a.prl"
            };
            let input = format!("{DEV_MANIFEST}\n[[recipes]]\noutput = \"{output}\"\n{source}");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("recipe"), "{error}");
        }
        let input = format!("{DEV_MANIFEST}\n[[recipes]]\noutput = \"levels/a.prl\"\n");
        assert!(Manifest::parse(&input).unwrap_err().contains("maps/"));
    }
}
