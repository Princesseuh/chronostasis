//! WPD container (`.wpd .bin .xwp .wdb .wpk .xfv .xgr .xwb`): a flat list of named members.

use std::path::Path;

use byteorder::{BigEndian as BE, ByteOrder};

use crate::{FormatError, Result};

const MAGIC: &[u8; 3] = b"WPD";
const HEADER_SIZE: usize = 16;
const RECORD_SIZE: usize = 32;
const NAME_LEN: usize = 16;
const ALIGN: usize = 4;
const ILLEGAL: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WpdEntry {
    /// At most 16 bytes on disk.
    pub name: String,
    /// Without the dot, at most 8 bytes.
    pub ext: String,
    pub data: Vec<u8>,
}

impl WpdEntry {
    /// Illegal characters stripped and `.ext` appended; this is what a mod matches on.
    pub fn file_name(&self) -> String {
        let stem: String = self.name.chars().filter(|c| !ILLEGAL.contains(c)).collect();
        if self.ext.is_empty() {
            stem
        } else {
            format!("{stem}.{}", self.ext)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wpd {
    pub entries: Vec<WpdEntry>,
}

impl Wpd {
    pub fn read(path: impl AsRef<Path>) -> Result<Wpd> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn parse(buf: &[u8]) -> Result<Wpd> {
        if buf.len() < HEADER_SIZE || &buf[0..3] != MAGIC {
            return Err(FormatError::BadMagic {
                expected: "WPD".into(),
                found: format!("{:?}", String::from_utf8_lossy(&buf[0..buf.len().min(4)])),
            });
        }
        let count = BE::read_u32(&buf[4..8]) as usize;
        // Each record needs 32 table bytes, so bound the count before allocating.
        if count
            .checked_mul(RECORD_SIZE)
            .and_then(|n| n.checked_add(HEADER_SIZE))
            .is_none_or(|n| n > buf.len())
        {
            return Err(malformed("record table truncated"));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let base = HEADER_SIZE + i * RECORD_SIZE;
            let rec = buf
                .get(base..base + RECORD_SIZE)
                .ok_or_else(|| malformed("record table truncated"))?;
            let name = field_str(&rec[0..NAME_LEN]);
            let off = BE::read_u32(&rec[16..20]) as usize;
            let size = BE::read_u32(&rec[20..24]) as usize;
            let ext = field_str(&rec[24..32]);
            let data = buf
                .get(off..off + size)
                .ok_or_else(|| malformed("member data out of range"))?
                .to_vec();
            entries.push(WpdEntry { name, ext, data });
        }
        Ok(Wpd { entries })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.write())?;
        Ok(())
    }

    pub fn write(&self) -> Vec<u8> {
        let count = self.entries.len();
        let table_end = HEADER_SIZE + count * RECORD_SIZE;
        let mut out = vec![0u8; table_end];
        out[0..4].copy_from_slice(b"WPD\0");
        BE::write_u32(&mut out[4..8], count as u32);

        let mut blob = Vec::new();
        let mut placement = Vec::with_capacity(count);
        for e in &self.entries {
            let off = table_end + blob.len();
            placement.push((off, e.data.len()));
            blob.extend_from_slice(&e.data);
            let pad = (ALIGN - blob.len() % ALIGN) % ALIGN;
            blob.resize(blob.len() + pad, 0u8);
        }

        for (i, e) in self.entries.iter().enumerate() {
            let base = HEADER_SIZE + i * RECORD_SIZE;
            write_field(&mut out[base..base + NAME_LEN], &e.name);
            BE::write_u32(&mut out[base + 16..base + 20], placement[i].0 as u32);
            BE::write_u32(&mut out[base + 20..base + 24], placement[i].1 as u32);
            write_field(&mut out[base + 24..base + 32], &e.ext);
        }
        out.extend_from_slice(&blob);
        out
    }

    pub fn find(&self, file_name: &str) -> Option<&WpdEntry> {
        self.entries.iter().find(|e| e.file_name() == file_name)
    }

    /// Matched by unpacked filename; returns whether one was found.
    pub fn set_member(&mut self, file_name: &str, data: Vec<u8>) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.file_name() == file_name) {
            e.data = data;
            true
        } else {
            false
        }
    }
}

fn field_str(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn write_field(dst: &mut [u8], s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(dst.len());
    dst[..n].copy_from_slice(&bytes[..n]);
    for b in &mut dst[n..] {
        *b = 0;
    }
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "WPD",
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let wpd = Wpd {
            entries: vec![
                WpdEntry {
                    name: "alpha".into(),
                    ext: "txb".into(),
                    data: vec![1, 2, 3],
                },
                WpdEntry {
                    name: "beta".into(),
                    ext: "".into(),
                    data: vec![9; 17],
                },
            ],
        };
        let bytes = wpd.write();
        let back = Wpd::parse(&bytes).unwrap();
        assert_eq!(wpd, back);
        assert_eq!(back.entries[0].file_name(), "alpha.txb");
        assert_eq!(back.entries[1].file_name(), "beta");
        let off1 = BE::read_u32(&bytes[16 + 16..16 + 20]);
        let off2 = BE::read_u32(&bytes[16 + 32 + 16..16 + 32 + 20]);
        assert_eq!(off1 % 4, 0);
        assert_eq!(off2 % 4, 0);
    }

    #[test]
    fn roundtrip_real_if_present() {
        let Ok(path) = std::env::var("FF13_TEST_WPD") else {
            return;
        };
        let orig = std::fs::read(&path).unwrap();
        let wpd = Wpd::parse(&orig).unwrap();
        let written = wpd.write();
        let reparsed = Wpd::parse(&written).unwrap();
        assert_eq!(wpd, reparsed, "reparse mismatch");
        eprintln!(
            "real WPD: {} members, orig {} bytes, rewritten {} bytes, byte-identical={}",
            wpd.entries.len(),
            orig.len(),
            written.len(),
            orig == written
        );
        for e in &wpd.entries {
            eprintln!("   {} ({} bytes)", e.file_name(), e.data.len());
        }
    }

    #[test]
    fn set_member_replaces() {
        let mut wpd = Wpd {
            entries: vec![WpdEntry {
                name: "x".into(),
                ext: "bin".into(),
                data: vec![0],
            }],
        };
        assert!(wpd.set_member("x.bin", vec![7, 7]));
        assert!(!wpd.set_member("missing.bin", vec![]));
        assert_eq!(wpd.find("x.bin").unwrap().data, vec![7, 7]);
    }
}
