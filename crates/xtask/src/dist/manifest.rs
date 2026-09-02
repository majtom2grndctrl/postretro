//! The committed distribution manifest schema.
//!
//! Paths intentionally remain slash-separated strings. The resolver compares a
//! recipe output directly with literals scanned from the emitted entry script.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Manifest {
    pub(crate) package: Package,
    pub(crate) recipes: Vec<Recipe>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) mod_root: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recipe {
    /// The mod-root-relative `maps/<name>.prl` literal emitted by the entry script.
    pub(crate) output: String,
    /// An optional workspace-relative `.map` source path.
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

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: String,
    mod_root: String,
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
            .map_err(|error| format!("read distribution manifest {}: {error}", path.display()))?;
        Self::parse(&contents)
            .map_err(|error| format!("parse distribution manifest {}: {error}", path.display()))
    }

    pub(crate) fn parse(contents: &str) -> Result<Self, String> {
        let raw: RawManifest =
            toml::from_str(contents).map_err(|error| format!("invalid TOML: {error}"))?;
        validate_package(&raw.package)?;

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
                mod_root: raw.package.mod_root,
            },
            recipes,
        })
    }
}

fn validate_package(package: &RawPackage) -> Result<(), String> {
    if !is_normal_component(&package.name) {
        return Err(format!(
            "package `{}`: name must be one normal path component",
            package.name
        ));
    }

    let components = slash_components(&package.mod_root).ok_or_else(|| {
        format!(
            "package mod_root `{}`: must use normal `/`-separated components",
            package.mod_root
        )
    })?;
    if components.len() != 2 {
        return Err(format!(
            "package mod_root `{}`: must contain exactly two components",
            package.mod_root
        ));
    }
    if components[0] == "dist" {
        return Err(format!(
            "package mod_root `{}`: first component may not be `dist`",
            package.mod_root
        ));
    }
    Ok(())
}

fn validate_recipe_path(path: &str, field: &str, label: &str) -> Result<(), String> {
    if slash_components(path).is_none() {
        return Err(format!(
            "{label}: {field} must be a workspace-relative `/` path"
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
        && !component.contains(['/', '\\'])
}

fn validate_args(args: &[String], label: &str) -> Result<Option<f32>, String> {
    let mut density = None;
    for (index, arg) in args.iter().enumerate() {
        if matches!(arg.as_str(), "-o" | "--release" | "--tui" | "--no-tui") {
            return Err(format!("{label}: args may not contain `{arg}`"));
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
        density =
            Some(value.parse::<f32>().map_err(|_| {
                format!("{label}: `--lightmap-density` value `{value}` is not an f32")
            })?);
    }
    Ok(density)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV_MANIFEST: &str = r#"
[package]
name = "postretro-dev"
mod_root = "content/dev"
"#;

    #[test]
    fn parses_minimal_manifest() {
        let manifest = Manifest::parse(DEV_MANIFEST).expect("manifest parses");
        assert_eq!(manifest.package.name, "postretro-dev");
        assert_eq!(manifest.package.mod_root, "content/dev");
        assert!(manifest.recipes.is_empty());
    }

    #[test]
    fn parses_recipe_and_density() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "dev"
mod_root = "content/dev"
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
    fn rejects_invalid_package_paths() {
        for (name, mod_root) in [
            (".", "content/dev"),
            ("nested/name", "content/dev"),
            ("dev", "content"),
            ("dev", "dist/packaged"),
        ] {
            let input = format!("[package]\nname = \"{name}\"\nmod_root = \"{mod_root}\"\n");
            assert!(Manifest::parse(&input).is_err(), "{input}");
        }
    }

    #[test]
    fn rejects_every_invalid_package_name_and_mod_root_shape() {
        for name in [".", "..", "nested/name", "nested\\\\name", ""] {
            let input = format!("[package]\nname = \"{name}\"\nmod_root = \"content/dev\"\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("package"), "{error}");
        }
        for mod_root in [
            "content",
            "content/dev/maps",
            "/content/dev",
            "content//dev",
            "content/../dev",
            "content\\\\dev",
            "dist/packaged",
        ] {
            let input = format!("[package]\nname = \"dev\"\nmod_root = \"{mod_root}\"\n");
            let error = Manifest::parse(&input).unwrap_err();
            assert!(error.contains("mod_root"), "{error}");
        }
    }

    #[test]
    fn rejects_duplicate_recipe_output() {
        let input = format!(
            "{DEV_MANIFEST}\n[[recipes]]\noutput = \"maps/a.prl\"\n\n[[recipes]]\noutput = \"maps/a.prl\"\n"
        );
        assert!(Manifest::parse(&input)
            .unwrap_err()
            .contains("recipe `maps/a.prl`"));
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
