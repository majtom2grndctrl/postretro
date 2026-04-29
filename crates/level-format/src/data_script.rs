// DataScript PRL section (ID 28): compiled data-script bytes + original source path.
// See: context/lib/build_pipeline.md §PRL section IDs · context/lib/scripting.md §Data context

use crate::FormatError;

/// Compiled data-script payload for a level.
///
/// Authored on `worldspawn` via the `data_script` KVP. The level compiler
/// resolves the path, compiles `.ts` to JS via `scripts-build` (Luau passes
/// through unchanged), and embeds the resulting bytes here. The runtime
/// evaluates `compiled_bytes` in a short-lived data context at level load
/// (see `context/lib/scripting.md` §Context Model).
///
/// `source_path` is the resolved absolute source path captured at compile time.
/// It is not consumed by the runtime today; it is reserved for the hot-reload
/// path so the file watcher can map back to the on-disk source.
///
/// On-disk layout (little-endian):
///   u32  source_path_byte_len
///   u8[] source_path_utf8       (no terminator, length == source_path_byte_len)
///   u32  compiled_bytes_len
///   u8[] compiled_bytes         (length == compiled_bytes_len)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataScriptSection {
    pub compiled_bytes: Vec<u8>,
    pub source_path: String,
}

impl DataScriptSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let path_bytes = self.source_path.as_bytes();
        let mut buf = Vec::with_capacity(8 + path_bytes.len() + self.compiled_bytes.len());
        buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(path_bytes);
        buf.extend_from_slice(&(self.compiled_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.compiled_bytes);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "data script section too short for source path length prefix",
            )));
        }
        let path_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let mut o = 4usize;
        if o + path_len > data.len() {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "data script section: truncated source path payload",
            )));
        }
        let source_path = std::str::from_utf8(&data[o..o + path_len])
            .map_err(|_| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "data script section: source path is not valid UTF-8",
                ))
            })?
            .to_string();
        o += path_len;

        if o + 4 > data.len() {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "data script section: truncated compiled bytes length prefix",
            )));
        }
        let bytes_len =
            u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
        o += 4;
        if o + bytes_len > data.len() {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "data script section: truncated compiled bytes payload",
            )));
        }
        let compiled_bytes = data[o..o + bytes_len].to_vec();

        Ok(Self {
            compiled_bytes,
            source_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_default() {
        let section = DataScriptSection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 8);
        let restored = DataScriptSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_populated() {
        let section = DataScriptSection {
            compiled_bytes: b"globalThis.foo = 1;\n".to_vec(),
            source_path: "/abs/path/to/level-data.ts".to_string(),
        };
        let bytes = section.to_bytes();
        let restored = DataScriptSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_unicode_path() {
        let section = DataScriptSection {
            compiled_bytes: vec![0x00, 0x01, 0x02, 0xFF],
            source_path: "/maps/écran/data.ts".to_string(),
        };
        let bytes = section.to_bytes();
        let restored = DataScriptSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = DataScriptSection::from_bytes(&[0u8; 3]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_truncated_path_payload() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&5u32.to_le_bytes()); // claim 5 path bytes
        buf.extend_from_slice(b"abc"); // only 3
        let err = DataScriptSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated source path payload"));
    }

    #[test]
    fn rejects_truncated_bytes_length_prefix() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(b"abc");
        // missing the 4-byte compiled-bytes length prefix
        let err = DataScriptSection::from_bytes(&buf).unwrap_err();
        assert!(
            err.to_string()
                .contains("truncated compiled bytes length prefix")
        );
    }

    #[test]
    fn rejects_truncated_bytes_payload() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes()); // empty path
        buf.extend_from_slice(&10u32.to_le_bytes()); // claim 10 compiled bytes
        buf.extend_from_slice(&[0u8; 4]); // only 4
        let err = DataScriptSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated compiled bytes payload"));
    }

    #[test]
    fn rejects_invalid_utf8_path() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.push(0xFF);
        buf.push(0xFE);
        buf.extend_from_slice(&0u32.to_le_bytes());
        let err = DataScriptSection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"));
    }
}
