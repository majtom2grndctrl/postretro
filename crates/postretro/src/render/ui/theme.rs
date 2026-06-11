// UI theme-token table: named color/font/spacing tokens widgets resolve against,
// the engine default theme, and the `ThemeDescriptor` wire form with per-token
// override merge. Pure data — no rendering, no taffy, no GPU.
// See: context/lib/ui.md

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Resolved theme: three flat token tables keyed by token name. Widgets (in
/// later tasks) reference tokens by name (`color: "critical"`); resolution sites
/// own fallback-and-warn, so the lookups here just return `Option`.
///
/// The required token set (the durable contract) is a FLOOR, not a closed list —
/// each map accepts arbitrary additional keys so a mod can add e.g. `cyan.500`.
/// Names are flat strings: `panel.default` is one literal color key (the dot is
/// part of the name, not a category prefix).
///
/// `HashMap` (not `BTreeMap`): tokens are looked up by name, never iterated in
/// order on any contract path. Iteration order is unobservable to callers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UiTheme {
    /// Token name → linear RGBA.
    colors: HashMap<String, [f32; 4]>,
    /// Token name → registered font family string.
    fonts: HashMap<String, String>,
    /// Token name → logical px.
    spacing: HashMap<String, f32>,
}

impl UiTheme {
    /// The engine default theme, carrying every required token with concrete
    /// cyberpunk-consistent values mirrored from the shipped splash constants
    /// (cyan accent, dark panel surface).
    ///
    /// The `body`/`mono` font entries name plain font-family strings. `"Inter"`
    /// matches `text::UI_FONT_FAMILY` and `"JetBrains Mono"` matches
    /// `text::UI_MONO_FONT_FAMILY` — both embedded faces are registered together
    /// in `build_font_system`.
    pub fn engine_default() -> Self {
        let colors = HashMap::from([
            // Cyberpunk warning palette. `critical` is a hot red, `warning` amber,
            // `ok` a green — all linear RGBA.
            ("critical".to_string(), [0.71, 0.05, 0.09, 1.0]),
            ("warning".to_string(), [0.85, 0.52, 0.05, 1.0]),
            ("ok".to_string(), [0.10, 0.62, 0.30, 1.0]),
            // `panel.default` is a literal flat key — the dark panel surface from
            // the splash (`splash::PANEL_COLOR`).
            ("panel.default".to_string(), [0.018, 0.026, 0.039, 1.0]),
        ]);

        let fonts = HashMap::from([
            ("body".to_string(), "Inter".to_string()),
            ("mono".to_string(), "JetBrains Mono".to_string()),
        ]);

        let spacing = HashMap::from([
            ("xs".to_string(), 2.0),
            ("s".to_string(), 4.0),
            ("m".to_string(), 8.0),
            ("l".to_string(), 16.0),
        ]);

        Self {
            colors,
            fonts,
            spacing,
        }
    }

    /// Resolve a color token. `None` when the name is absent — the resolution
    /// site decides the fallback.
    pub fn color(&self, name: &str) -> Option<[f32; 4]> {
        self.colors.get(name).copied()
    }

    /// Resolve a font-family token, borrowed.
    pub fn font(&self, name: &str) -> Option<&str> {
        self.fonts.get(name).map(String::as_str)
    }

    /// Resolve a spacing token (logical px).
    pub fn spacing(&self, name: &str) -> Option<f32> {
        self.spacing.get(name).copied()
    }

    /// Merge a `ThemeDescriptor` override over this theme, per TOKEN (not per
    /// category): start from a clone of `self`, then overwrite ONLY the tokens
    /// the override names. An override naming one color leaves every other color
    /// untouched, and a name not in the default is added (mods extend the table).
    ///
    /// The engine-side theme setter (`Renderer::set_ui_theme`) installs an
    /// already-merged `UiTheme`; the override-document → merge path is the wire
    /// contract overrides serialize through, but its production caller (script
    /// ingestion) is deferred — so it is test-only on a release build today.
    ///
    /// Paired deferred-G1 entry points (both `#[allow(dead_code)]` until script
    /// ingestion lands): this method and `Renderer::set_ui_theme` in `render/mod.rs`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_override(&self, over: &ThemeDescriptor) -> Self {
        let mut merged = self.clone();
        for (name, value) in &over.colors {
            merged.colors.insert(name.clone(), *value);
        }
        for (name, value) in &over.fonts {
            merged.fonts.insert(name.clone(), value.clone());
        }
        for (name, value) in &over.spacing {
            merged.spacing.insert(name.clone(), *value);
        }
        merged
    }
}

/// Serde wire form for a theme override document. Each category map is optional
/// and defaults to empty, so an override may name only the tokens it changes
/// (the merge is per-token). Locked deliverable: script ingestion is deferred,
/// but this format is the contract overrides serialize through.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ThemeDescriptor {
    #[serde(default)]
    pub colors: HashMap<String, [f32; 4]>,
    #[serde(default)]
    pub fonts: HashMap<String, String>,
    #[serde(default)]
    pub spacing: HashMap<String, f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linear-RGBA approximate equality — colors are floating-point, so never
    /// compare exact (testing guide §3 Floating-point comparison).
    fn colors_close(a: [f32; 4], b: [f32; 4]) -> bool {
        const EPS: f32 = 1e-6;
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < EPS)
    }

    #[test]
    fn engine_default_contains_all_required_token_names() {
        let theme = UiTheme::engine_default();
        for name in ["critical", "warning", "ok", "panel.default"] {
            assert!(theme.color(name).is_some(), "missing color token {name:?}");
        }
        for name in ["body", "mono"] {
            assert!(theme.font(name).is_some(), "missing font token {name:?}");
        }
        for name in ["xs", "s", "m", "l"] {
            assert!(
                theme.spacing(name).is_some(),
                "missing spacing token {name:?}"
            );
        }
    }

    #[test]
    fn default_font_tokens_name_the_coordinated_families() {
        // The embedded Inter and JetBrains Mono faces register these exact family names; the contract pins them.
        let theme = UiTheme::engine_default();
        assert_eq!(theme.font("body"), Some("Inter"));
        assert_eq!(theme.font("mono"), Some("JetBrains Mono"));
    }

    #[test]
    fn theme_descriptor_round_trips_through_serde_json() {
        let json = r#"{"colors":{"critical":[1.0,0.0,0.0,1.0]},"fonts":{"body":"Inter"},"spacing":{"m":8.0}}"#;
        let desc: ThemeDescriptor = serde_json::from_str(json).expect("must deserialize");
        // Re-deserialize the re-serialized form rather than comparing JSON text:
        // HashMap iteration order is unspecified, so a byte-identical round-trip
        // is not a contract here — value identity is.
        let reserialized = serde_json::to_string(&desc).expect("must serialize");
        let roundtripped: ThemeDescriptor =
            serde_json::from_str(&reserialized).expect("must re-deserialize");
        assert_eq!(desc, roundtripped);
        assert!(colors_close(desc.colors["critical"], [1.0, 0.0, 0.0, 1.0]));
        assert_eq!(desc.fonts["body"], "Inter");
        assert_eq!(desc.spacing["m"], 8.0);
    }

    #[test]
    fn empty_theme_descriptor_deserializes_with_all_categories_defaulted() {
        // An override that names nothing is a valid document: every category map
        // defaults to empty, so merging it changes no tokens.
        let desc: ThemeDescriptor = serde_json::from_str("{}").expect("must deserialize");
        assert!(desc.colors.is_empty());
        assert!(desc.fonts.is_empty());
        assert!(desc.spacing.is_empty());
    }

    #[test]
    fn override_replaces_named_token_and_leaves_siblings_at_default() {
        // Per-token, NOT per-category: overriding ONLY `critical` must leave the
        // other default colors (and all fonts/spacing) intact.
        let default = UiTheme::engine_default();
        let over = ThemeDescriptor {
            colors: HashMap::from([("critical".to_string(), [0.0, 1.0, 1.0, 1.0])]),
            ..Default::default()
        };
        let merged = default.with_override(&over);

        // The named token took the override value.
        assert!(colors_close(
            merged.color("critical").unwrap(),
            [0.0, 1.0, 1.0, 1.0]
        ));
        // Sibling colors survive from the default — not wiped by the category.
        assert!(colors_close(
            merged.color("warning").unwrap(),
            default.color("warning").unwrap()
        ));
        assert!(colors_close(
            merged.color("panel.default").unwrap(),
            default.color("panel.default").unwrap()
        ));
        // Untouched categories survive entirely.
        assert_eq!(merged.font("body"), default.font("body"));
        assert_eq!(merged.spacing("l"), default.spacing("l"));
    }

    #[test]
    fn override_adds_extra_token_beyond_required_set() {
        // The required set is a floor: a mod's `cyan.500` round-trips through the
        // wire form, merges into the resolved theme, and is retrievable.
        let json = r#"{"colors":{"cyan.500":[0.1,0.55,0.62,1.0]}}"#;
        let desc: ThemeDescriptor = serde_json::from_str(json).expect("must deserialize");
        assert!(colors_close(
            desc.colors["cyan.500"],
            [0.1, 0.55, 0.62, 1.0]
        ));

        let merged = UiTheme::engine_default().with_override(&desc);
        // The extra token is retrievable...
        assert!(colors_close(
            merged.color("cyan.500").unwrap(),
            [0.1, 0.55, 0.62, 1.0]
        ));
        // ...and the required tokens still resolve alongside it.
        assert!(merged.color("critical").is_some());
    }

    #[test]
    fn unknown_token_resolves_to_none() {
        // A name not in any map resolves to None — resolution sites own the
        // fallback-and-warn, so the table itself never invents a value.
        let theme = UiTheme::engine_default();
        assert_eq!(theme.color("nonexistent"), None);
        assert_eq!(theme.font("nonexistent"), None);
        assert_eq!(theme.spacing("nonexistent"), None);
    }
}
