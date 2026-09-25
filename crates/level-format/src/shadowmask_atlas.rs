// ShadowmaskAtlas PRL section (ID 42): per-selected-light baked visibility masks.
// Governing context: context/lib/build_pipeline.md

use crate::FormatError;

pub const SHADOWMASK_CHANNEL_DROPPED: u8 = 0xFF;

/// BC5 `.rg`, two mask groups side by side in each layer. The texture is
/// `2 × width` wide: group 0 (slots 0/1) fills the left half, group 1 (slots
/// 2/3) the right. ASCII `SMB5`, so no width a pre-tag compiler wrote can
/// equal it.
pub const SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE: u32 = u32::from_le_bytes(*b"SMB5");

/// Mask groups per texel under `SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE`.
pub const SHADOWMASK_GROUP_COUNT: u32 = 2;

const BC5_BLOCK_BYTES: usize = 16;
const HEADER_BYTES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasSection {
    /// Payload encoding; `SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE` is the only value.
    pub format: u32,
    /// Lightmap atlas width. The texture width is `width × SHADOWMASK_GROUP_COUNT`.
    pub width: u32,
    pub height: u32,
    pub layer_count: u32,
    /// One entry per EntityShadowLights selection index: slot `0..3`, or
    /// 0xFF when that selected light was globally dropped from the mask.
    /// Slot `s` addresses group `s / 2`, channel `s % 2`.
    pub channels: Vec<u8>,
    /// Layer-major BC5 blocks (BC4 endpoint/selector data per channel):
    /// per layer, one `2·width × height` plane in row-major 4×4 blocks.
    /// Decoded channel value 255 means fully visible.
    pub data: Vec<u8>,
}

/// A `ShadowmaskAtlasSection` without its payload: the dimensions and slot
/// table a loaded level keeps after the GPU upload takes the BC5 blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasHeader {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub layer_count: u32,
    pub channels: Vec<u8>,
}

impl ShadowmaskAtlasHeader {
    /// Width of the GPU texture that holds both groups side by side.
    pub fn texture_width(&self) -> Option<u32> {
        self.width.checked_mul(SHADOWMASK_GROUP_COUNT)
    }
}

impl ShadowmaskAtlasSection {
    /// Width of the GPU texture that holds both groups side by side.
    pub fn texture_width(&self) -> Option<u32> {
        self.width.checked_mul(SHADOWMASK_GROUP_COUNT)
    }

    /// Split into the header a loaded level keeps and the payload only the
    /// GPU upload reads. Moves the payload; copies nothing.
    pub fn into_parts(self) -> (ShadowmaskAtlasHeader, Vec<u8>) {
        let Self {
            format,
            width,
            height,
            layer_count,
            channels,
            data,
        } = self;
        (
            ShadowmaskAtlasHeader {
                format,
                width,
                height,
                layer_count,
                channels,
            },
            data,
        )
    }

    /// Payload bytes for one layer at these lightmap dimensions.
    pub fn layer_payload_len(width: u32, height: u32) -> Option<usize> {
        let blocks_x = (width as usize)
            .checked_mul(SHADOWMASK_GROUP_COUNT as usize)?
            .div_ceil(4);
        let blocks_y = (height as usize).div_ceil(4);
        blocks_x.checked_mul(blocks_y)?.checked_mul(BC5_BLOCK_BYTES)
    }

    /// Payload bytes for a whole atlas at these dimensions.
    pub fn payload_len(width: u32, height: u32, layer_count: u32) -> Option<usize> {
        Self::layer_payload_len(width, height)?.checked_mul(layer_count as usize)
    }

    pub fn byte_len(&self) -> usize {
        self.header_len() + self.data.len()
    }

    /// Header, slot table and padding: everything before `data`. The streamed
    /// memo writer shares this with `to_bytes`, so warm and cold headers
    /// cannot drift apart.
    pub fn header_bytes(&self) -> Vec<u8> {
        let selected_light_count = self.channels.len() as u32;
        let mut out = Vec::with_capacity(self.header_len());
        out.extend_from_slice(&self.format.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.layer_count.to_le_bytes());
        out.extend_from_slice(&selected_light_count.to_le_bytes());
        out.extend_from_slice(&self.channels);
        out.extend(std::iter::repeat_n(0u8, padding_to_4(self.channels.len())));
        out
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.header_bytes();
        out.reserve_exact(self.data.len());
        out.extend_from_slice(&self.data);
        out
    }

    /// Parses a tagged section. When the tagged parse fails and the bytes
    /// form a valid pre-tag raw `Rgba8Unorm` section, the error names the
    /// format mismatch rather than whichever structural check tripped first.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        parse_tagged(data).map_err(|err| {
            if is_pre_tag_raw_section(data) {
                invalid_data(
                    "shadowmask atlas format mismatch: payload is the retired untagged raw \
                     Rgba8Unorm layout, not BC5; re-bake the level",
                )
            } else {
                err
            }
        })
    }

    fn header_len(&self) -> usize {
        HEADER_BYTES + self.channels.len() + padding_to_4(self.channels.len())
    }
}

fn parse_tagged(data: &[u8]) -> crate::Result<ShadowmaskAtlasSection> {
    if data.len() < HEADER_BYTES {
        return Err(invalid_eof("shadowmask atlas section too short for header"));
    }

    let format = read_u32(data, 0);
    if format != SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE {
        return Err(invalid_data(format!(
            "shadowmask atlas format tag {format:#010x} is unknown"
        )));
    }
    let width = read_u32(data, 4);
    let height = read_u32(data, 8);
    let layer_count = read_u32(data, 12);
    let selected_light_count = read_u32(data, 16) as usize;
    if width == 0 || height == 0 || layer_count == 0 {
        return Err(invalid_data(format!(
            "shadowmask atlas dimensions {width}x{height}x{layer_count} must be nonzero"
        )));
    }
    if width % 4 != 0 || height % 4 != 0 {
        return Err(invalid_data(format!(
            "shadowmask atlas dimensions {width}x{height} are not multiples of 4"
        )));
    }

    let payload_start = HEADER_BYTES
        .checked_add(selected_light_count)
        .and_then(|n| n.checked_add(padding_to_4(selected_light_count)))
        .ok_or_else(|| invalid_data("shadowmask atlas channel table overflows"))?;
    if data.len() < payload_start {
        return Err(invalid_eof(
            "shadowmask atlas section truncated in channel table",
        ));
    }

    let expected_payload = ShadowmaskAtlasSection::payload_len(width, height, layer_count)
        .ok_or_else(|| invalid_data("shadowmask atlas payload size overflows"))?;
    let actual_payload = data.len() - payload_start;
    if actual_payload != expected_payload {
        return Err(invalid_data(format!(
            "shadowmask atlas payload has {actual_payload} bytes, expected {expected_payload}"
        )));
    }

    let channels = data[HEADER_BYTES..HEADER_BYTES + selected_light_count].to_vec();
    if let Some(&channel) = channels.iter().find(|&&channel| !is_valid_slot(channel)) {
        return Err(invalid_data(format!(
            "shadowmask atlas slot {channel} is not 0..3 or 0xFF"
        )));
    }

    Ok(ShadowmaskAtlasSection {
        format,
        width,
        height,
        layer_count,
        channels,
        data: data[payload_start..].to_vec(),
    })
}

/// Whether `data` is a structurally valid section in the retired untagged
/// layout: `u32` width, height, layer_count, selected_light_count, the slot
/// table padded to 4, then `width × height × layer_count × 4` raw bytes.
fn is_pre_tag_raw_section(data: &[u8]) -> bool {
    const PRE_TAG_HEADER_BYTES: usize = 16;
    if data.len() < PRE_TAG_HEADER_BYTES {
        return false;
    }
    let width = read_u32(data, 0) as usize;
    let height = read_u32(data, 4) as usize;
    let layer_count = read_u32(data, 8) as usize;
    let selected_light_count = read_u32(data, 12) as usize;
    let Some(payload_start) = PRE_TAG_HEADER_BYTES
        .checked_add(selected_light_count)
        .and_then(|n| n.checked_add(padding_to_4(selected_light_count)))
        .filter(|&start| start <= data.len())
    else {
        return false;
    };
    let expected_payload = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(layer_count))
        .and_then(|n| n.checked_mul(4));
    expected_payload == Some(data.len() - payload_start)
        && data[PRE_TAG_HEADER_BYTES..PRE_TAG_HEADER_BYTES + selected_light_count]
            .iter()
            .all(|&channel| is_valid_slot(channel))
}

fn is_valid_slot(channel: u8) -> bool {
    channel <= 3 || channel == SHADOWMASK_CHANNEL_DROPPED
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("caller checked the header length"),
    )
}

fn padding_to_4(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

fn invalid_eof(message: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        message.into(),
    ))
}

fn invalid_data(message: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn section(
        width: u32,
        height: u32,
        layer_count: u32,
        channels: Vec<u8>,
    ) -> ShadowmaskAtlasSection {
        let payload_len = ShadowmaskAtlasSection::payload_len(width, height, layer_count).unwrap();
        ShadowmaskAtlasSection {
            format: SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
            width,
            height,
            layer_count,
            channels,
            data: (0..payload_len).map(|i| (i % 251) as u8).collect(),
        }
    }

    /// The retired layout, byte for byte as the pre-tag compiler wrote it.
    fn pre_tag_raw_bytes(width: u32, height: u32, layer_count: u32, channels: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&layer_count.to_le_bytes());
        out.extend_from_slice(&(channels.len() as u32).to_le_bytes());
        out.extend_from_slice(channels);
        out.extend(std::iter::repeat_n(0u8, padding_to_4(channels.len())));
        out.extend(std::iter::repeat_n(
            255u8,
            width as usize * height as usize * layer_count as usize * 4,
        ));
        out
    }

    fn error_message(bytes: &[u8]) -> String {
        ShadowmaskAtlasSection::from_bytes(bytes)
            .expect_err("section must be rejected")
            .to_string()
    }

    #[test]
    fn shadowmask_atlas_round_trips_empty_single_and_full_slot_tables() {
        for channels in [
            vec![],
            vec![2],
            vec![0, 1, 2, 3],
            vec![3, SHADOWMASK_CHANNEL_DROPPED, 0, 1, 2],
        ] {
            let section = section(8, 4, 2, channels);
            let bytes = section.to_bytes();
            assert_eq!(section.byte_len(), bytes.len());
            assert_eq!(bytes[..section.header_len()], section.header_bytes()[..]);
            assert_eq!(ShadowmaskAtlasSection::from_bytes(&bytes).unwrap(), section);
        }
    }

    #[test]
    fn shadowmask_atlas_payload_is_half_the_raw_rgba_arithmetic() {
        let (width, height, layer_count) = (64u32, 32u32, 3u32);
        let raw = (width * height * layer_count * 4) as usize;
        assert_eq!(
            ShadowmaskAtlasSection::payload_len(width, height, layer_count),
            Some(raw / 2)
        );
        assert_eq!(section(64, 32, 3, vec![0]).texture_width(), Some(128));
    }

    #[test]
    fn shadowmask_atlas_rejects_length_alignment_tag_and_slot_errors() {
        let good = section(8, 4, 2, vec![0, 3]);

        let mut short = good.to_bytes();
        short.pop();
        assert!(error_message(&short).contains("expected"));
        let mut long = good.to_bytes();
        long.extend_from_slice(&[0; 16]);
        assert!(error_message(&long).contains("expected"));

        for (width, height) in [(6, 4), (8, 5)] {
            let mut misaligned = good.clone();
            misaligned.width = width;
            misaligned.height = height;
            assert!(
                error_message(&misaligned.to_bytes()).contains("not multiples of 4"),
                "{width}x{height} must be rejected for alignment"
            );
        }

        let mut unknown = good.clone();
        unknown.format = SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE + 1;
        assert!(error_message(&unknown.to_bytes()).contains("format tag"));

        let mut bad_slot = good.clone();
        bad_slot.channels = vec![4];
        assert!(error_message(&bad_slot.to_bytes()).contains("slot 4"));
    }

    #[test]
    fn shadowmask_atlas_rejects_pre_tag_raw_payload_as_format_mismatch() {
        for channels in [
            vec![0u8],
            vec![0, 1, 2, 3],
            vec![SHADOWMASK_CHANNEL_DROPPED; 5],
        ] {
            let bytes = pre_tag_raw_bytes(64, 64, 2, &channels);
            assert!(
                error_message(&bytes).contains("format mismatch"),
                "pre-tag payload with {} slot(s) must be rejected by format",
                channels.len()
            );
        }
    }

    // Pin: stale-payload-tag-collision. A pre-tag payload whose first word
    // reads as the valid tag must still be named a format mismatch, never
    // fall through to a length or dimension error.
    #[test]
    fn shadowmask_atlas_rejects_pre_tag_payload_whose_tag_word_collides() {
        for (height, layer_count) in [(0, 1), (64, 0)] {
            let bytes = pre_tag_raw_bytes(
                SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
                height,
                layer_count,
                &[0, 1],
            );
            assert_eq!(read_u32(&bytes, 0), SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE);
            assert!(
                error_message(&bytes).contains("format mismatch"),
                "collision {height}x{layer_count} must be rejected by format"
            );
        }
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::ShadowmaskAtlas as u32, 42);
        assert_eq!(SectionId::from_u32(42), Some(SectionId::ShadowmaskAtlas));
    }
}
