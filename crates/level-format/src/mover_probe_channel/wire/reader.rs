// MoverProbeChannel (id 57) byte cursor and little-endian helpers: every
// read is length-checked before it allocates, so no count read from the file
// reserves memory its bytes do not back.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::Error;

pub(super) fn put_u32s(out: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

pub(in crate::mover_probe_channel) fn put_halves(out: &mut Vec<u8>, halves: &[u16]) {
    for half in halves {
        out.extend_from_slice(&half.to_le_bytes());
    }
}

pub(super) fn checked_add(total: usize, add: usize, what: &'static str) -> Result<usize, Error> {
    total.checked_add(add).ok_or(Error::SizeOverflow { what })
}

pub(super) fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

pub(super) fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

pub(super) fn le_f32(bytes: &[u8]) -> f32 {
    f32::from_le_bytes(bytes.try_into().unwrap())
}

/// A cursor that refuses to read past the end, so no count read from the
/// file allocates before its bytes are known to exist.
pub(super) struct Reader<'a> {
    pub(super) data: &'a [u8],
    pub(super) at: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let remaining = self.data.len() - self.at;
        if len > remaining {
            return Err(Error::Truncated {
                what,
                needed: len,
                remaining,
            });
        }
        let bytes = &self.data[self.at..self.at + len];
        self.at += len;
        Ok(bytes)
    }

    pub(super) fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(self.take(1, what)?[0])
    }

    pub(super) fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(le_u32(self.take(4, what)?))
    }

    /// `count` fixed-size records, length-checked as one read.
    pub(super) fn records(
        &mut self,
        count: usize,
        size: usize,
        what: &'static str,
    ) -> Result<std::slice::ChunksExact<'a, u8>, Error> {
        let len = count
            .checked_mul(size)
            .ok_or(Error::SizeOverflow { what })?;
        Ok(self.take(len, what)?.chunks_exact(size))
    }

    pub(super) fn halves(&mut self, count: usize, what: &'static str) -> Result<Vec<u16>, Error> {
        Ok(self
            .records(count, 2, what)?
            .map(|half| u16::from_le_bytes([half[0], half[1]]))
            .collect())
    }

    /// A CSR offset table over `rows`: starts at 0, never decreases.
    pub(super) fn offsets(
        &mut self,
        rows: usize,
        block: &'static str,
        what: &'static str,
    ) -> Result<Vec<u32>, Error> {
        let count = rows.checked_add(1).ok_or(Error::SizeOverflow { what })?;
        let offsets: Vec<u32> = self.records(count, 4, what)?.map(le_u32).collect();
        if offsets[0] != 0 {
            return Err(Error::CsrOffsets {
                block,
                index: 0,
                offset: offsets[0],
                previous: 0,
            });
        }
        if let Some(index) = offsets.windows(2).position(|pair| pair[0] > pair[1]) {
            return Err(Error::CsrOffsets {
                block,
                index: index + 1,
                offset: offsets[index + 1],
                previous: offsets[index],
            });
        }
        Ok(offsets)
    }
}
