//! Variable-length integers for the posting lists.

pub(crate) fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// A cursor over a byte slice.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn varint(&mut self) -> Option<u64> {
        let (mut value, mut shift) = (0u64, 0u32);
        loop {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.varint().and_then(|v| u32::try_from(v).ok())
    }

    pub fn byte(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trip() {
        let values = [0u64, 1, 127, 128, 300, 70_000, u32::MAX as u64, u64::MAX];
        let mut buf = Vec::new();
        for v in values {
            put_varint(&mut buf, v);
        }
        let mut r = Reader::new(&buf);
        for v in values {
            assert_eq!(r.varint(), Some(v));
        }
        assert_eq!(r.varint(), None);
    }
}
