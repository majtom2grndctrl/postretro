// Body-presence flag shared by the whole-load SH sections (ids 27, 34, 35, 41, 45).
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;
use crate::SectionId;
use crate::animated_direct_sh_delta_volumes::{
    ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION, AnimatedDirectShDeltaVolumesSection,
};
use crate::delta_sh_volumes::{DELTA_SH_VOLUMES_VERSION, DeltaShVolumesSection};
use crate::direct_sh_delta_volumes::{
    DIRECT_SH_DELTA_VOLUMES_VERSION, DirectShDeltaVolumesSection,
};
use crate::direct_sh_volume::{DIRECT_SH_VOLUME_VERSION, DirectShVolumeSection};
use crate::sh_volume::{OctahedralShVolumeSection, SH_VOLUME_VERSION};

/// Whether a whole-load SH section carries its body (atlas or delta tiles).
///
/// Only the whole-load path (`POSTRETRO_SH_STREAMING=off`, or a PRL without id 50)
/// reads the bodies; the streaming loader
/// takes payload from id 50. An omitted body keeps every length and count
/// field at its full value; only the body bytes are absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShBodyPresence {
    Present,
    Omitted,
}

impl ShBodyPresence {
    /// Wire value of the body-presence field: 1 present, 0 omitted.
    pub const fn flag(self) -> u8 {
        match self {
            Self::Present => 1,
            Self::Omitted => 0,
        }
    }

    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

/// The five sections that carry a body-presence field.
pub const SH_BODY_SECTIONS: [SectionId; 5] = [
    SectionId::DeltaShVolumes,
    SectionId::OctahedralShVolume,
    SectionId::DirectShVolume,
    SectionId::DirectShDeltaVolumes,
    SectionId::AnimatedDirectShDeltaVolumes,
];

/// Version and flag share one width per section: `u32` for ids 34/35, `u8`
/// for the sparse delta sections. The flag is the last field of the fixed
/// header, so it ends at `header_len`.
struct FlagLayout {
    header_len: usize,
    field_width: usize,
    expected_version: u32,
}

fn flag_layout(section: SectionId) -> Option<FlagLayout> {
    let (header_len, field_width, expected_version) = match section {
        SectionId::DeltaShVolumes => (
            DeltaShVolumesSection::FIXED_HEADER_SIZE,
            1,
            u32::from(DELTA_SH_VOLUMES_VERSION),
        ),
        SectionId::OctahedralShVolume => {
            (OctahedralShVolumeSection::HEADER_SIZE, 4, SH_VOLUME_VERSION)
        }
        SectionId::DirectShVolume => (
            DirectShVolumeSection::HEADER_SIZE,
            4,
            DIRECT_SH_VOLUME_VERSION,
        ),
        SectionId::DirectShDeltaVolumes => (
            DirectShDeltaVolumesSection::FIXED_HEADER_SIZE,
            1,
            u32::from(DIRECT_SH_DELTA_VOLUMES_VERSION),
        ),
        SectionId::AnimatedDirectShDeltaVolumes => (
            AnimatedDirectShDeltaVolumesSection::FIXED_HEADER_SIZE,
            1,
            u32::from(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION),
        ),
        _ => return None,
    };
    Some(FlagLayout {
        header_len,
        field_width,
        expected_version,
    })
}

/// Bytes of section prefix [`read_sh_body_presence`] needs: the fixed header
/// through the body-presence field. `None` for a section without the field.
pub fn sh_body_header_len(section: SectionId) -> Option<usize> {
    flag_layout(section).map(|layout| layout.header_len)
}

/// Read the body-presence field from the start of a section's bytes.
///
/// `header_prefix` holds at least [`sh_body_header_len`] bytes from the start
/// of the section. Checks the section-internal version first, since the field
/// only sits at this offset in the current version.
pub fn read_sh_body_presence(
    section: SectionId,
    header_prefix: &[u8],
) -> crate::Result<ShBodyPresence> {
    let layout = flag_layout(section).ok_or_else(|| {
        FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("section {} has no SH body-presence field", section as u32),
        ))
    })?;
    if header_prefix.len() < layout.header_len {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "section {} header truncated: need {} bytes for the body-presence field, got {}",
                section as u32,
                layout.header_len,
                header_prefix.len()
            ),
        )));
    }
    let version = read_field(header_prefix, 0, layout.field_width);
    if version != layout.expected_version {
        return Err(invalid_data(format!(
            "section {} internal version {version}, expected {} — recompile the .prl with the current `prl-build`",
            section as u32, layout.expected_version
        )));
    }
    let flag = read_field(
        header_prefix,
        layout.header_len - layout.field_width,
        layout.field_width,
    );
    decode_sh_body_flag(section, flag)
}

/// Decode a body-presence field value; anything but 0 or 1 is invalid data.
pub(crate) fn decode_sh_body_flag(section: SectionId, flag: u32) -> crate::Result<ShBodyPresence> {
    match flag {
        1 => Ok(ShBodyPresence::Present),
        0 => Ok(ShBodyPresence::Omitted),
        other => Err(invalid_data(format!(
            "section {} body-presence flag {other} is invalid: expected 1 (present) or 0 (omitted)",
            section as u32
        ))),
    }
}

/// Whole-section decoders need the body: an omitted one fails by name.
pub(crate) fn require_sh_body(section: SectionId, flag: u32) -> crate::Result<()> {
    match decode_sh_body_flag(section, flag)? {
        ShBodyPresence::Present => Ok(()),
        ShBodyPresence::Omitted => Err(FormatError::ShBodyOmitted {
            section_id: section as u32,
        }),
    }
}

fn read_field(data: &[u8], at: usize, width: usize) -> u32 {
    match width {
        1 => u32::from(data[at]),
        4 => u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]),
        _ => unreachable!("body-presence fields are u8 or u32"),
    }
}

fn invalid_data(msg: String) -> FormatError {
    FormatError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

/// Shared wire checks for each section's body-presence tests.
#[cfg(test)]
pub(crate) fn assert_sh_body_wire_forms(
    section: SectionId,
    present: &[u8],
    omitted: &[u8],
    body: std::ops::Range<usize>,
    decode: impl Fn(&[u8]) -> crate::Result<()>,
) {
    assert!(!body.is_empty(), "fixture must carry a non-empty body");
    assert_eq!(
        read_sh_body_presence(section, present).unwrap(),
        ShBodyPresence::Present
    );
    assert_eq!(
        read_sh_body_presence(section, omitted).unwrap(),
        ShBodyPresence::Omitted
    );

    // Omitted form is the present form minus the body bytes, flag cleared.
    let layout = flag_layout(section).unwrap();
    let flag_start = layout.header_len - layout.field_width;
    let mut expected = present[..body.start].to_vec();
    expected[flag_start] = ShBodyPresence::Omitted.flag();
    expected.extend_from_slice(&present[body.end..]);
    assert_eq!(
        omitted,
        expected.as_slice(),
        "omitted form is metadata + tail"
    );

    decode(present).expect("present form decodes");
    let err = decode(omitted).expect_err("omitted form must not decode whole");
    assert!(
        matches!(err, FormatError::ShBodyOmitted { section_id } if section_id == section as u32),
        "expected ShBodyOmitted for {section:?}, got: {err}"
    );

    let mut bad_flag = present.to_vec();
    bad_flag[flag_start] = 2;
    let msg = decode(&bad_flag)
        .expect_err("flag 2 is invalid")
        .to_string();
    assert!(msg.contains("body-presence flag 2"), "{section:?}: {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_for(section: SectionId, body: ShBodyPresence) -> Vec<u8> {
        let layout = flag_layout(section).unwrap();
        let mut bytes = vec![0u8; layout.header_len];
        bytes[..layout.field_width]
            .copy_from_slice(&layout.expected_version.to_le_bytes()[..layout.field_width]);
        bytes[layout.header_len - layout.field_width] = body.flag();
        bytes
    }

    #[test]
    fn sh_body_header_len_names_each_section_header_through_the_flag() {
        for section in SH_BODY_SECTIONS {
            let expected = match section {
                SectionId::DeltaShVolumes => 27,
                SectionId::OctahedralShVolume => 88,
                SectionId::DirectShVolume => 80,
                SectionId::DirectShDeltaVolumes => 23,
                SectionId::AnimatedDirectShDeltaVolumes => 27,
                _ => unreachable!(),
            };
            assert_eq!(sh_body_header_len(section), Some(expected), "{section:?}");
        }
        assert_eq!(sh_body_header_len(SectionId::ClusterShPayloads), None);
    }

    #[test]
    fn read_sh_body_presence_reads_both_flag_values_for_every_section() {
        for section in SH_BODY_SECTIONS {
            for body in [ShBodyPresence::Present, ShBodyPresence::Omitted] {
                let prefix = prefix_for(section, body);
                assert_eq!(read_sh_body_presence(section, &prefix).unwrap(), body);
            }
        }
    }

    #[test]
    fn read_sh_body_presence_rejects_flag_other_than_zero_or_one() {
        for section in SH_BODY_SECTIONS {
            let mut prefix = prefix_for(section, ShBodyPresence::Present);
            let flag_start = prefix.len() - flag_layout(section).unwrap().field_width;
            prefix[flag_start] = 2;
            let msg = read_sh_body_presence(section, &prefix)
                .unwrap_err()
                .to_string();
            assert!(msg.contains("body-presence flag"), "{section:?}: {msg}");
        }
    }

    #[test]
    fn read_sh_body_presence_rejects_stale_version_and_short_prefix() {
        for section in SH_BODY_SECTIONS {
            let mut prefix = prefix_for(section, ShBodyPresence::Present);
            prefix[0] -= 1;
            let msg = read_sh_body_presence(section, &prefix)
                .unwrap_err()
                .to_string();
            assert!(msg.contains("recompile"), "{section:?}: {msg}");

            let prefix = prefix_for(section, ShBodyPresence::Present);
            let msg = read_sh_body_presence(section, &prefix[..prefix.len() - 1])
                .unwrap_err()
                .to_string();
            assert!(msg.contains("truncated"), "{section:?}: {msg}");
        }
    }

    #[test]
    fn read_sh_body_presence_refuses_a_section_without_the_field() {
        let err = read_sh_body_presence(SectionId::ClusterShPayloads, &[0; 128]).unwrap_err();
        assert!(err.to_string().contains("no SH body-presence field"));
    }

    #[test]
    fn omitted_body_error_names_section_and_rebake_remedy() {
        let err = require_sh_body(SectionId::DirectShVolume, 0).unwrap_err();
        assert!(matches!(err, FormatError::ShBodyOmitted { section_id: 35 }));
        let msg = err.to_string();
        assert!(
            msg.contains("section 35")
                && msg.contains("prl-build --whole-load-sh")
                && msg.contains("POSTRETRO_SH_STREAMING=off"),
            "{msg}"
        );
    }
}
