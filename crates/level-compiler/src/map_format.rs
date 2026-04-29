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

impl std::str::FromStr for MapFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idtech2" => Ok(Self::IdTech2),
            "idtech3" => Ok(Self::IdTech3),
            "idtech4" => Ok(Self::IdTech4),
            other => Err(format!(
                "unknown map format '{other}'; expected idtech2, idtech3, or idtech4"
            )),
        }
    }
}

impl MapFormat {
    /// False for variants whose parsers are not yet implemented.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::IdTech2)
    }

    /// Scale factor from map-native units to engine meters.
    ///
    /// Apply to vertex positions and plane distances. Do NOT apply to normals —
    /// normals are direction vectors; only the axis swizzle applies to them.
    ///
    /// For IdTech2: 1 Quake unit = 0.0254 m (exact, since 1 inch = 0.0254 m).
    pub fn units_to_meters(&self) -> f64 {
        match self {
            Self::IdTech2 => 0.0254,
            // IdTech3 and IdTech4 are unsupported — unreachable in practice
            // because is_supported() gates entry. Add their scales when implemented.
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
