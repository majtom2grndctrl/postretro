// The mod author's optional `ModManifest.input` block, as both SDK drains
// produce it. Structural only: the engine owns the command, input, and
// activator vocabulary and validates against it.
// See: context/lib/scripting.md · context/lib/input.md §2

/// The mod author's optional input block, drained structurally. Engine
/// vocabulary (command IDs, input names, activators) is validated by the
/// engine, not here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModInputBlock {
    /// Commands in authored order. JS keeps object key order. Luau tables carry
    /// no key order, so the Luau drain sorts commands by command ID.
    pub commands: Vec<ModInputCommand>,
    pub glyphs: ModInputGlyphs,
}

/// One authored command entry, keyed in the manifest by its command ID.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModInputCommand {
    pub id: String,
    pub label: Option<String>,
    pub category: Option<String>,
    pub order: Option<f64>,
    /// `Some(true)` forces the command shown, `Some(false)` hidden; `None`
    /// leaves relevance to derivation.
    pub show: Option<bool>,
    /// `None` = the key is absent (engine default applies); `Some(vec![])` = unbound.
    pub keyboard_mouse: Option<Vec<ModInputBinding>>,
    pub gamepad: Option<Vec<ModInputBinding>>,
}

/// One authored default binding. A malformed entry keeps its slot with an
/// empty `input`, a `Some("")` activator, or a NaN threshold, so the engine
/// diagnoses it and falls back for that command and device class.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModInputBinding {
    pub input: String,
    pub activator: Option<String>,
    /// Seconds: a tap's max or a hold's min.
    pub threshold: Option<f64>,
}

/// Glyph art directory per device family; the asset id is `<dir>/<input>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModInputGlyphs {
    pub keyboard_mouse: Option<String>,
    pub xbox: Option<String>,
    pub playstation: Option<String>,
    pub nintendo: Option<String>,
}
