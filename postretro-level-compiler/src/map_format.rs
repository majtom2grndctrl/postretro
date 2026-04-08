// Map format variant selection for the level compiler.
// See: context/lib/build_pipeline.md §PRL

/// Identifies which .map dialect the compiler should expect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapFormat {
    /// Quake 1 / Quake 2 brush format (default).
    IdTech2,
    /// Quake 3 format — includes bezier patches. Not yet supported.
    IdTech3,
    /// Doom 3 format — includes meshDef / brushDef3. Not yet supported.
    IdTech4,
}

pub const DEFAULT_MAP_FORMAT: MapFormat = MapFormat::IdTech2;

impl MapFormat {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "idtech2" => Ok(Self::IdTech2),
            "idtech3" => Ok(Self::IdTech3),
            "idtech4" => Ok(Self::IdTech4),
            other => Err(format!("unknown map format '{other}'; expected idtech2, idtech3, or idtech4")),
        }
    }

    /// False for variants whose parsers are not yet implemented.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::IdTech2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_parses_idtech2() {
        assert_eq!(MapFormat::from_str("idtech2").unwrap(), MapFormat::IdTech2);
    }

    #[test]
    fn from_str_parses_idtech3() {
        assert_eq!(MapFormat::from_str("idtech3").unwrap(), MapFormat::IdTech3);
    }

    #[test]
    fn from_str_parses_idtech4() {
        assert_eq!(MapFormat::from_str("idtech4").unwrap(), MapFormat::IdTech4);
    }

    #[test]
    fn from_str_rejects_unknown_format() {
        let result = MapFormat::from_str("bogus");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("unknown map format"), "got: {msg}");
    }

    #[test]
    fn idtech2_is_supported() {
        assert!(MapFormat::IdTech2.is_supported());
    }

    #[test]
    fn idtech3_is_not_supported() {
        assert!(!MapFormat::IdTech3.is_supported());
    }

    #[test]
    fn idtech4_is_not_supported() {
        assert!(!MapFormat::IdTech4.is_supported());
    }

    #[test]
    fn default_format_is_idtech2() {
        assert_eq!(DEFAULT_MAP_FORMAT, MapFormat::IdTech2);
    }
}
