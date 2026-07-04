// ShadowmaskAtlas PRL section (ID 42): per-selected-light baked visibility masks.
// See: context/plans/in-progress/static-light-shadowmask-world-receipt/index.md

use crate::FormatError;

pub const SHADOWMASK_CHANNEL_DROPPED: u8 = 0xFF;
pub const SHADOWMASK_TEXEL_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowmaskAtlasSection {
    pub width: u32,
    pub height: u32,
    pub layer_count: u32,
    /// One entry per EntityShadowLights selection index: 0..3 for RGBA, or
    /// 0xFF when that selected light was globally dropped from the mask.
    pub channels: Vec<u8>,
    /// Layer-major Rgba8Unorm payload. 255 means fully visible.
    pub data: Vec<u8>,
}

impl ShadowmaskAtlasSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let selected_light_count = self.channels.len() as u32;
        let channel_pad = padding_to_4(self.channels.len());
        let mut out = Vec::with_capacity(16 + self.channels.len() + channel_pad + self.data.len());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.layer_count.to_le_bytes());
        out.extend_from_slice(&selected_light_count.to_le_bytes());
        out.extend_from_slice(&self.channels);
        out.extend(std::iter::repeat_n(0u8, channel_pad));
        out.extend_from_slice(&self.data);
        out
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 16 {
            return Err(invalid_eof("shadowmask atlas section too short for header"));
        }

        let width = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let layer_count = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let selected_light_count = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
        let channel_pad = padding_to_4(selected_light_count);
        let payload_start = 16usize
            .checked_add(selected_light_count)
            .and_then(|n| n.checked_add(channel_pad))
            .ok_or_else(|| invalid_data("shadowmask atlas channel table overflows"))?;
        if data.len() < payload_start {
            return Err(invalid_eof(
                "shadowmask atlas section truncated in channel table",
            ));
        }

        let expected_payload = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(layer_count as usize))
            .and_then(|n| n.checked_mul(SHADOWMASK_TEXEL_BYTES))
            .ok_or_else(|| invalid_data("shadowmask atlas payload size overflows"))?;
        let actual_payload = data.len() - payload_start;
        if actual_payload != expected_payload {
            return Err(invalid_data(format!(
                "shadowmask atlas payload has {actual_payload} bytes, expected {expected_payload}"
            )));
        }

        let channels = data[16..16 + selected_light_count].to_vec();
        for &channel in &channels {
            if channel > 3 && channel != SHADOWMASK_CHANNEL_DROPPED {
                return Err(invalid_data(format!(
                    "shadowmask atlas channel {channel} is not 0..3 or 0xFF"
                )));
            }
        }

        Ok(Self {
            width,
            height,
            layer_count,
            channels,
            data: data[payload_start..].to_vec(),
        })
    }
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

    #[test]
    fn round_trip_multi_layer_payload_and_channel_padding() {
        let section = ShadowmaskAtlasSection {
            width: 2,
            height: 2,
            layer_count: 2,
            channels: vec![0, 2, SHADOWMASK_CHANNEL_DROPPED],
            data: (0..32).collect(),
        };

        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 16 + 4 + 32);
        let restored = ShadowmaskAtlasSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored, section);
    }

    #[test]
    fn rejects_bad_payload_length_and_channel() {
        let section = ShadowmaskAtlasSection {
            width: 1,
            height: 1,
            layer_count: 1,
            channels: vec![4],
            data: vec![255; 4],
        };
        assert!(ShadowmaskAtlasSection::from_bytes(&section.to_bytes()).is_err());

        let section = ShadowmaskAtlasSection {
            width: 1,
            height: 1,
            layer_count: 1,
            channels: vec![0],
            data: vec![255; 3],
        };
        assert!(ShadowmaskAtlasSection::from_bytes(&section.to_bytes()).is_err());
    }

    #[test]
    fn section_id_is_pinned() {
        assert_eq!(SectionId::ShadowmaskAtlas as u32, 42);
        assert_eq!(SectionId::from_u32(42), Some(SectionId::ShadowmaskAtlas));
    }
}
