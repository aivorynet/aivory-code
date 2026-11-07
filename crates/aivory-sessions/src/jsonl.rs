//! Head and tail reading for large JSON-lines files.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// The first and the last bytes of a file. For a small file `head` holds the whole file.
#[derive(Debug, Default)]
pub(crate) struct HeadTail {
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    /// `true` when `head` holds the whole file and `tail` is empty.
    pub whole: bool,
}

impl HeadTail {
    /// Complete lines of the head. The last line is dropped when it can be cut off.
    pub fn head_lines(&self) -> impl Iterator<Item = &[u8]> {
        let mut parts: Vec<&[u8]> = self.head.split(|b| *b == b'\n').collect();
        if !self.whole {
            parts.pop();
        }
        parts.into_iter().filter(|l| !l.is_empty())
    }

    /// Complete lines of the tail. The first line is dropped because it can be cut off.
    pub fn tail_lines(&self) -> impl Iterator<Item = &[u8]> {
        let mut parts = self.tail.split(|b| *b == b'\n');
        parts.next();
        parts.filter(|l| !l.is_empty())
    }
}

/// Read at most `head_len` bytes from the start and `tail_len` bytes from the end.
pub(crate) fn read_head_tail(
    path: &Path,
    size: u64,
    head_len: usize,
    tail_len: usize,
) -> io::Result<HeadTail> {
    let mut file = File::open(path)?;
    if size <= (head_len + tail_len) as u64 {
        let mut head = Vec::with_capacity(size as usize);
        file.read_to_end(&mut head)?;
        return Ok(HeadTail {
            head,
            tail: Vec::new(),
            whole: true,
        });
    }
    let mut head = vec![0u8; head_len];
    file.read_exact(&mut head)?;
    let mut tail = vec![0u8; tail_len];
    file.seek(SeekFrom::Start(size - tail_len as u64))?;
    file.read_exact(&mut tail)?;
    Ok(HeadTail {
        head,
        tail,
        whole: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cut_lines() {
        let ht = HeadTail {
            head: b"a\nb\nc".to_vec(),
            tail: b"x\ny\nz\n".to_vec(),
            whole: false,
        };
        assert_eq!(
            ht.head_lines().collect::<Vec<_>>(),
            vec![b"a".as_slice(), b"b".as_slice()]
        );
        assert_eq!(
            ht.tail_lines().collect::<Vec<_>>(),
            vec![b"y".as_slice(), b"z".as_slice()]
        );
        let whole = HeadTail {
            head: b"a\nb".to_vec(),
            tail: Vec::new(),
            whole: true,
        };
        assert_eq!(whole.head_lines().count(), 2);
    }
}
