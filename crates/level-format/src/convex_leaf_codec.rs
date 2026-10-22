// Shared little-endian codec of the convex-leaf sections (ids 52, 55, 56):
// bounded reader, scalar and string writers, and the id-52 brush encoding.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;
use crate::environment_volumes::{EnvironmentBrush, EnvironmentPlane, MIN_PLANES_PER_BRUSH};

/// One brush with zero planes: AABB min, AABB max, plane count.
pub(crate) const MIN_BRUSH_SIZE: usize = 12 + 12 + 4;
/// One plane: normal and distance.
pub(crate) const PLANE_SIZE: usize = 16;

/// Encoded size of `brushes` as [`write_brushes`] writes them, count included.
pub(crate) fn brushes_byte_len(brushes: &[EnvironmentBrush]) -> usize {
    4 + brushes
        .iter()
        .map(|brush| MIN_BRUSH_SIZE + PLANE_SIZE * brush.planes.len())
        .sum::<usize>()
}

pub(crate) fn invalid<T>(section: &str, message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("{section}: {message}"),
    )))
}

pub(crate) fn write_count(out: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).expect("PRL section counts fit the u32 field");
    out.extend_from_slice(&n.to_le_bytes());
}

pub(crate) fn write_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn write_vec3(out: &mut Vec<u8>, value: [f32; 3]) {
    for v in value {
        write_f32(out, v);
    }
}

/// A u32 byte length, then the UTF-8 bytes.
pub(crate) fn write_string(out: &mut Vec<u8>, value: &str) {
    write_count(out, value.len());
    out.extend_from_slice(value.as_bytes());
}

/// u32 brush count, then per brush: AABB min and max, u32 plane count, and
/// each plane's normal and distance.
pub(crate) fn write_brushes(out: &mut Vec<u8>, brushes: &[EnvironmentBrush]) {
    write_count(out, brushes.len());
    for brush in brushes {
        write_vec3(out, brush.aabb_min);
        write_vec3(out, brush.aabb_max);
        write_count(out, brush.planes.len());
        for plane in &brush.planes {
            write_vec3(out, plane.normal);
            write_f32(out, plane.distance);
        }
    }
}

/// Reads untrusted section bytes. Every error names `section`.
pub(crate) struct SectionReader<'a> {
    data: &'a [u8],
    offset: usize,
    section: &'static str,
}

impl<'a> SectionReader<'a> {
    pub(crate) fn new(data: &'a [u8], section: &'static str) -> Self {
        Self {
            data,
            offset: 0,
            section,
        }
    }

    pub(crate) fn invalid<T>(&self, message: String) -> crate::Result<T> {
        invalid(self.section, message)
    }

    /// Reject any bytes left after the last field.
    pub(crate) fn finish(&self) -> crate::Result<()> {
        if self.offset != self.data.len() {
            return self.invalid(format!(
                "trailing bytes: expected {}, got {}",
                self.offset,
                self.data.len()
            ));
        }
        Ok(())
    }

    fn take(&mut self, n: usize, what: &str) -> crate::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("{}: truncated {what}", self.section),
                ))
            })?;
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self, what: &str) -> crate::Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &str) -> crate::Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self, what: &str) -> crate::Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn finite_f32(&mut self, what: &str) -> crate::Result<f32> {
        let value = f32::from_bits(self.u32(what)?);
        if !value.is_finite() {
            return self.invalid(format!("{what} is not finite"));
        }
        Ok(value)
    }

    pub(crate) fn finite_vec3(&mut self, what: &str) -> crate::Result<[f32; 3]> {
        Ok([
            self.finite_f32(what)?,
            self.finite_f32(what)?,
            self.finite_f32(what)?,
        ])
    }

    pub(crate) fn flag(&mut self, what: &str) -> crate::Result<bool> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            other => self.invalid(format!("{what} must be 0 or 1, got {other}")),
        }
    }

    /// A count whose elements each need at least `min_element_size` bytes,
    /// rejected before allocation when the remaining bytes cannot hold it.
    pub(crate) fn bounded_count(
        &mut self,
        what: &str,
        min_element_size: usize,
    ) -> crate::Result<usize> {
        let n = self.u32(what)? as usize;
        let remaining = self.data.len() - self.offset;
        if n > remaining / min_element_size {
            return self.invalid(format!(
                "{what} {n} exceeds what the remaining {remaining} bytes can hold"
            ));
        }
        Ok(n)
    }

    pub(crate) fn string(&mut self, what: &str) -> crate::Result<String> {
        let n = self.u32(&format!("{what} length"))? as usize;
        let bytes = self.take(n, what)?;
        match std::str::from_utf8(bytes) {
            Ok(value) => Ok(value.to_owned()),
            Err(_) => self.invalid(format!("invalid UTF-8 in {what}")),
        }
    }

    /// The convex leaves [`write_brushes`] writes. `owner` prefixes every
    /// message ("volume 3", "record 0"). At least one brush, each a closed
    /// leaf: AABB min <= max, at least four planes, no zero normal.
    pub(crate) fn brushes(&mut self, owner: &str) -> crate::Result<Vec<EnvironmentBrush>> {
        let count = self.bounded_count(&format!("{owner} brush count"), MIN_BRUSH_SIZE)?;
        if count == 0 {
            return self.invalid(format!("{owner} has no brushes"));
        }
        let mut brushes = Vec::with_capacity(count);
        for b in 0..count {
            let what = format!("{owner} brush {b}");
            let aabb_min = self.finite_vec3(&format!("{what} AABB min"))?;
            let aabb_max = self.finite_vec3(&format!("{what} AABB max"))?;
            if aabb_min.iter().zip(aabb_max).any(|(min, max)| *min > max) {
                return self.invalid(format!("{what} AABB min exceeds max"));
            }
            let plane_count = self.bounded_count(&format!("{what} plane count"), PLANE_SIZE)?;
            if plane_count < MIN_PLANES_PER_BRUSH {
                return self.invalid(format!(
                    "{what} has {plane_count} planes; a closed convex leaf needs at least \
                     {MIN_PLANES_PER_BRUSH} (four planes bound a tetrahedron)"
                ));
            }
            let mut planes = Vec::with_capacity(plane_count);
            for p in 0..plane_count {
                let normal = self.finite_vec3(&format!("{what} plane {p} normal"))?;
                if normal == [0.0; 3] {
                    return self.invalid(format!("{what} plane {p} has a zero plane normal"));
                }
                let distance = self.finite_f32(&format!("{what} plane {p} distance"))?;
                planes.push(EnvironmentPlane { normal, distance });
            }
            brushes.push(EnvironmentBrush {
                aabb_min,
                aabb_max,
                planes,
            });
        }
        Ok(brushes)
    }
}
