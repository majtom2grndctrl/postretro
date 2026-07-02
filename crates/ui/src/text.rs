// CPU text helpers for UI layout and font registration.
// See: context/lib/ui.md §1, §5

use std::path::Path;

pub use cosmic_text::FontSystem;
use cosmic_text::{Attrs, Buffer as TextBuffer, Family, Metrics, Shaping};

/// Engine default UI typeface: Inter (SIL Open Font License 1.1).
const UI_FONT_TTF: &[u8] = include_bytes!("../../../content/base/fonts/Inter-Regular.ttf");

/// Font family name inside `UI_FONT_TTF`.
pub const UI_FONT_FAMILY: &str = "Inter";

/// Engine default UI monospace typeface: JetBrains Mono (SIL Open Font License 1.1).
const UI_MONO_FONT_TTF: &[u8] =
    include_bytes!("../../../content/base/fonts/JetBrainsMono-Regular.ttf");

/// Font family name inside `UI_MONO_FONT_TTF`.
pub const UI_MONO_FONT_FAMILY: &str = "JetBrains Mono";

/// cosmic-text shapes against a `Metrics { font_size, line_height }`.
pub const LINE_HEIGHT_FACTOR: f32 = 1.25;

/// Build a `FontSystem` with the embedded Inter (body) and JetBrains Mono (mono)
/// faces registered.
pub fn build_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system.db_mut().load_font_data(UI_FONT_TTF.to_vec());
    font_system
        .db_mut()
        .load_font_data(UI_MONO_FONT_TTF.to_vec());
    font_system
}

/// Whether `family` resolves to a face the `FontSystem` database registered.
pub fn font_family_is_registered(font_system: &FontSystem, family: &str) -> bool {
    font_system
        .db()
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == family))
}

/// Read a runtime UI font file (TTF/OTF) from disk into owned bytes.
pub fn read_font_file(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Measure a single text run's intrinsic size from real shaped-glyph metrics.
pub fn measure_run(
    font_system: &mut FontSystem,
    content: &str,
    font_size: f32,
    family: &str,
) -> (f32, f32) {
    let line_height = font_size * LINE_HEIGHT_FACTOR;
    let metrics = Metrics::new(font_size, line_height);
    let mut buffer = TextBuffer::new(font_system, metrics);
    buffer.set_size(font_system, None, None);
    buffer.set_text(
        font_system,
        content,
        &Attrs::new().family(Family::Name(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);

    let mut width = 0.0_f32;
    let mut height = 0.0_f32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height += run.line_height;
    }
    if height == 0.0 {
        height = line_height;
    }
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_bytes_are_present_and_a_truetype() {
        assert!(
            UI_FONT_TTF.len() > 1024,
            "embedded TTF looks truncated ({} bytes)",
            UI_FONT_TTF.len(),
        );
        let magic = &UI_FONT_TTF[0..4];
        assert!(
            magic == [0x00, 0x01, 0x00, 0x00]
                || magic == *b"OTTO"
                || magic == *b"true"
                || magic == *b"ttcf",
            "embedded font is not a recognized sfnt/TrueType (magic {magic:?})",
        );
    }

    #[test]
    fn embedded_font_registers_and_resolves_family() {
        let mut fs = FontSystem::new();
        fs.db_mut().load_font_data(UI_FONT_TTF.to_vec());
        assert!(font_family_is_registered(&fs, UI_FONT_FAMILY));
    }

    #[test]
    fn embedded_mono_font_bytes_are_present_and_a_truetype() {
        assert!(
            UI_MONO_FONT_TTF.len() > 1024,
            "embedded mono TTF looks truncated ({} bytes)",
            UI_MONO_FONT_TTF.len(),
        );
        let magic = &UI_MONO_FONT_TTF[0..4];
        assert!(
            magic == [0x00, 0x01, 0x00, 0x00]
                || magic == *b"OTTO"
                || magic == *b"true"
                || magic == *b"ttcf",
            "embedded mono font is not a recognized sfnt/TrueType (magic {magic:?})",
        );
    }

    #[test]
    fn embedded_mono_font_registers_and_resolves_family() {
        let mut fs = FontSystem::new();
        fs.db_mut().load_font_data(UI_MONO_FONT_TTF.to_vec());
        assert!(font_family_is_registered(&fs, UI_MONO_FONT_FAMILY));
    }

    #[test]
    fn build_font_system_registers_both_primary_and_mono_families() {
        let fs = build_font_system();
        assert!(font_family_is_registered(&fs, UI_FONT_FAMILY));
        assert!(font_family_is_registered(&fs, UI_MONO_FONT_FAMILY));
    }

    #[test]
    fn mono_and_primary_families_measure_to_different_widths() {
        let mut fs = build_font_system();
        let content = "iiiiWWWW mmmm";
        let font_size = 24.0_f32;
        let (body_w, _) = measure_run(&mut fs, content, font_size, UI_FONT_FAMILY);
        let (mono_w, _) = measure_run(&mut fs, content, font_size, UI_MONO_FONT_FAMILY);
        const EPS: f32 = 1.0;
        assert!(
            (body_w - mono_w).abs() > EPS,
            "mono ({mono_w}) and primary ({body_w}) widths should differ beyond {EPS}px",
        );
    }

    fn workspace_font(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("content/base/fonts")
            .join(name)
    }

    #[test]
    fn read_font_file_reads_ttf_bytes_from_disk() {
        let bytes =
            read_font_file(&workspace_font("Inter-Regular.ttf")).expect("fixture font reads");
        assert_eq!(&bytes[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn read_font_file_missing_path_errors_for_caller_to_skip() {
        let err = read_font_file(Path::new("/nonexistent/mod/font.ttf"))
            .expect_err("missing font path must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn runtime_loaded_font_bytes_register_family() {
        let mut fs = FontSystem::new();
        assert!(!font_family_is_registered(&fs, UI_MONO_FONT_FAMILY));
        let bytes =
            read_font_file(&workspace_font("JetBrainsMono-Regular.ttf")).expect("mono font reads");
        fs.db_mut().load_font_data(bytes);
        assert!(font_family_is_registered(&fs, UI_MONO_FONT_FAMILY));
    }

    #[test]
    fn family_lookup_rejects_mismatched_declared_name() {
        let mut fs = FontSystem::new();
        let bytes = read_font_file(&workspace_font("Inter-Regular.ttf")).expect("Inter reads");
        fs.db_mut().load_font_data(bytes);
        assert!(!font_family_is_registered(&fs, "NotTheRealFamilyName"));
        assert!(font_family_is_registered(&fs, UI_FONT_FAMILY));
    }
}
