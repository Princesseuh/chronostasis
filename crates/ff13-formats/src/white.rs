//! White archive: a `filelist*.win32.bin` index over a `white_img*.win32.bin` data blob.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use byteorder::{LittleEndian as LE, ReadBytesExt};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rayon::prelude::*;
use std::io::Write;

use crate::{FormatError, Game, Result};

const SECTOR: u64 = 2048;

/// One indexed file: where its bytes live in `white_img` and how big they are.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub file_code: u32,
    /// XIII-2/LR only.
    pub type_id: Option<u8>,
    /// The filelist chunk this entry's path record lives in.
    pub chunk: u16,
    /// Virtual, `/`-separated; empty for "no path" files.
    pub path: String,
    /// Into `white_img`.
    pub offset: u64,
    pub uncmp_size: u32,
    pub cmp_size: u32,
}

impl FileEntry {
    pub fn is_compressed(&self) -> bool {
        self.uncmp_size != self.cmp_size
    }
}

#[derive(Debug, Clone)]
pub struct Filelist {
    pub game: Game,
    pub encrypted: bool,
    pub entries: Vec<FileEntry>,
}

impl Filelist {
    pub fn read(path: impl AsRef<Path>, game: Game) -> Result<Filelist> {
        Self::parse(&std::fs::read(path)?, game)
    }

    /// An encrypted XIII-2/LR index is transparently decrypted first.
    pub fn parse(data: &[u8], game: Game) -> Result<Filelist> {
        let encrypted = data.len() >= 24
            && u32::from_le_bytes(data[20..24].try_into().unwrap()) == crate::crypto::ENC_TAG;

        // Decrypted bytes outlive the borrow below; the header stays as a 32-byte prefix.
        let decrypted;
        let (data, base) = if encrypted {
            decrypted = crate::crypto::decrypt_filelist(data)?;
            (decrypted.as_slice(), 32usize)
        } else {
            (data, 0usize)
        };

        let mut hdr = &data[base..];
        let info_off = hdr.read_u32::<LE>()? as usize + base;
        let data_off = hdr.read_u32::<LE>()? as usize + base;
        let total_files = hdr.read_u32::<LE>()? as usize;
        let entries_start = base + 12;

        if data_off < info_off || data_off > data.len() {
            return Err(malformed("filelist", "chunk section offsets out of range"));
        }
        let total_chunks = (data_off - info_off) / 12;

        let mut chunks: Vec<Vec<u8>> = Vec::with_capacity(total_chunks);
        for i in 0..total_chunks {
            let rec = info_off + i * 12;
            let mut c = &data[rec..rec + 12];
            let _uncmp = c.read_u32::<LE>()?;
            let cmp = c.read_u32::<LE>()? as usize;
            let start = c.read_u32::<LE>()? as usize;
            let comp = data
                .get(data_off + start..data_off + start + cmp)
                .ok_or_else(|| malformed("filelist", "chunk data out of range"))?;
            let mut out = Vec::new();
            ZlibDecoder::new(comp).read_to_end(&mut out)?;
            chunks.push(out);
        }

        let mut entries = Vec::with_capacity(total_files);
        let mut current_chunk: i64 = -1;
        for i in 0..total_files {
            let e = entries_start + i * 8;
            let mut c = data
                .get(e..e + 8)
                .ok_or_else(|| malformed("filelist", "entry out of range"))?;
            let file_code = c.read_u32::<LE>()?;

            let (chunk_num, path_pos, type_id) = if game.code() == 1 {
                let chunk = c.read_u16::<LE>()? as usize;
                let pos = c.read_u16::<LE>()? as usize;
                (chunk, pos, None)
            } else {
                let mut pos = c.read_u16::<LE>()? as i32;
                let _raw_chunk = c.read_u8()?;
                let type_id = c.read_u8()?;
                if pos == 0 {
                    current_chunk += 1;
                } else if pos == 0x8000 {
                    current_chunk += 1;
                    pos -= 0x8000;
                } else if pos > 0x8000 {
                    pos -= 0x8000;
                }
                (current_chunk as usize, pos as usize, Some(type_id))
            };

            let buf = chunks
                .get(chunk_num)
                .ok_or_else(|| malformed("filelist", "chunk index out of range"))?;
            let record = read_cstr(buf, path_pos)?;
            entries.push(parse_record(file_code, type_id, chunk_num as u16, &record)?);
        }

        Ok(Filelist {
            game,
            encrypted,
            entries,
        })
    }

    /// The minimum size the paired `white_img` must be.
    pub fn max_extent(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.offset + e.cmp_size as u64)
            .max()
            .unwrap_or(0)
    }

    pub fn extract(&self, white_img: &mut File, entry: &FileEntry) -> Result<Vec<u8>> {
        self.read_entry(white_img, entry)
    }

    /// Takes `&File` so one handle can serve parallel workers without a shared cursor.
    fn read_entry(&self, img: &File, entry: &FileEntry) -> Result<Vec<u8>> {
        let mut stored = vec![0u8; entry.cmp_size as usize];
        read_exact_at(img, &mut stored, entry.offset)?;
        if entry.is_compressed() {
            let mut out = Vec::with_capacity(entry.uncmp_size as usize);
            ZlibDecoder::new(&stored[..]).read_to_end(&mut out)?;
            Ok(out)
        } else {
            Ok(stored)
        }
    }

    /// Mirrors the virtual tree under `out_dir`.
    pub fn unpack_all(
        &self,
        white_img: impl AsRef<Path>,
        out_dir: impl AsRef<Path>,
    ) -> Result<usize> {
        self.unpack_inner(white_img.as_ref(), out_dir.as_ref(), None)
    }

    /// As [`Self::unpack_all`], but bumps `progress` by one per file written.
    pub fn unpack_all_with_progress(
        &self,
        white_img: impl AsRef<Path>,
        out_dir: impl AsRef<Path>,
        progress: &AtomicUsize,
    ) -> Result<usize> {
        self.unpack_inner(white_img.as_ref(), out_dir.as_ref(), Some(progress))
    }

    /// The single source of truth for where an unpack writes, so unpack and its inverse agree.
    fn resolve_targets(&self, out_dir: &Path) -> Vec<(&FileEntry, std::path::PathBuf)> {
        // The noPath counter is order-dependent, so resolve sequentially.
        let mut no_path = 0u32;
        self.entries
            .iter()
            .map(|entry| {
                let rel = if entry.path.is_empty() {
                    no_path += 1;
                    format!("noPath/FILE_{no_path}")
                } else {
                    entry.path.clone()
                };
                (entry, safe_join(out_dir, &rel))
            })
            .collect()
    }

    /// Never `out_dir` itself or above, so a revert prunes exactly what the unpack made.
    pub fn unpacked_dirs(&self, out_dir: &Path) -> Vec<std::path::PathBuf> {
        let mut dirs = std::collections::HashSet::new();
        for (_, dest) in self.resolve_targets(out_dir) {
            let mut cur = dest.parent();
            while let Some(dir) = cur {
                if dir == out_dir || !dir.starts_with(out_dir) {
                    break;
                }
                // Already recorded means its ancestors are too.
                if !dirs.insert(dir.to_path_buf()) {
                    break;
                }
                cur = dir.parent();
            }
        }
        dirs.into_iter().collect()
    }

    /// The precise inverse of [`Self::unpack_all`]. Missing files are skipped; returns how many went.
    pub fn remove_unpacked(&self, out_dir: &Path, progress: Option<&AtomicUsize>) -> usize {
        let mut removed = 0;
        for (_, dest) in self.resolve_targets(out_dir) {
            if std::fs::remove_file(&dest).is_ok() {
                removed += 1;
            }
            if let Some(p) = progress {
                p.fetch_add(1, Ordering::Relaxed);
            }
        }
        removed
    }

    fn unpack_inner(
        &self,
        white_img: &Path,
        out_dir: &Path,
        progress: Option<&AtomicUsize>,
    ) -> Result<usize> {
        let img = File::open(white_img)?;
        let targets = self.resolve_targets(out_dir);

        // Create each parent dir once up front, so the parallel writers don't all
        // race on create_dir_all for the same directories.
        let mut seen = std::collections::HashSet::new();
        for (_, dest) in &targets {
            if let Some(parent) = dest.parent()
                && seen.insert(parent)
            {
                std::fs::create_dir_all(parent)?;
            }
        }

        targets
            .par_iter()
            .try_for_each(|(entry, dest)| -> Result<()> {
                let bytes = self.read_entry(&img, entry)?;
                std::fs::write(dest, &bytes)?;
                if let Some(p) = progress {
                    p.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })?;
        Ok(self.entries.len())
    }

    /// FFXIII only; preserves entry order, codes and chunk assignment, changing only offsets and sizes.
    ///
    /// Both outputs land on temp siblings and are renamed into place only once complete, so a
    /// mid-run failure never truncates an existing multi-GB archive.
    pub fn repack(
        &self,
        extracted_dir: impl AsRef<Path>,
        out_white_img: impl AsRef<Path>,
        out_filelist: impl AsRef<Path>,
    ) -> Result<()> {
        if self.game != Game::XIII {
            return Err(FormatError::Unsupported(
                "white_img repack supports FFXIII only".into(),
            ));
        }
        let (out_white_img, out_filelist) = (out_white_img.as_ref(), out_filelist.as_ref());
        let sources = self.resolve_targets(extracted_dir.as_ref());
        for (_, src) in &sources {
            if !src.is_file() {
                return Err(malformed(
                    "filelist",
                    &format!("missing loose file {}", src.display()),
                ));
            }
        }

        let tmp_img = tmp_sibling(out_white_img);
        let tmp_fl = tmp_sibling(out_filelist);
        if let Err(e) = self.repack_to(&sources, &tmp_img, &tmp_fl) {
            let _ = std::fs::remove_file(&tmp_img);
            let _ = std::fs::remove_file(&tmp_fl);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp_img, out_white_img) {
            let _ = std::fs::remove_file(&tmp_img);
            let _ = std::fs::remove_file(&tmp_fl);
            return Err(e.into());
        }
        // The archive is already swapped; keep the matching filelist so the pair can be repaired.
        std::fs::rename(&tmp_fl, out_filelist).map_err(|e| {
            malformed(
                "filelist",
                &format!(
                    "archive written but filelist rename failed ({e}); move {} to {} to finish",
                    tmp_fl.display(),
                    out_filelist.display()
                ),
            )
        })
    }

    fn repack_to(
        &self,
        sources: &[(&FileEntry, std::path::PathBuf)],
        out_white_img: &Path,
        out_filelist: &Path,
    ) -> Result<()> {
        let num_chunks = self.entries.iter().map(|e| e.chunk).max().unwrap_or(0) as usize + 1;
        let mut chunk_data: Vec<Vec<u8>> = vec![Vec::new(); num_chunks];
        let mut entry_meta: Vec<(u32, u16, u16)> = Vec::with_capacity(self.entries.len());

        let mut img = std::io::BufWriter::new(File::create(out_white_img)?);
        let mut img_len: u64 = 0;

        for (e, src) in sources {
            let data = std::fs::read(src)?;
            let uncmp = data.len() as u32;
            let stored = if e.is_compressed() {
                zlib_compress(&data)
            } else {
                data
            };
            let cmp = stored.len() as u32;

            let pad = (SECTOR - img_len % SECTOR) % SECTOR;
            if pad > 0 {
                img.write_all(&vec![0u8; pad as usize])?;
                img_len += pad;
            }
            let file_pos = img_len / SECTOR;
            img.write_all(&stored)?;
            img_len += stored.len() as u64;

            let path_pos = u16::try_from(chunk_data[e.chunk as usize].len())
                .map_err(|_| malformed("filelist", "chunk path records exceed 64 KiB"))?;
            let record_path = if e.path.is_empty() { " " } else { &e.path };
            let record = format!("{file_pos:x}:{uncmp:x}:{cmp:x}:{record_path}\0");
            chunk_data[e.chunk as usize].extend_from_slice(record.as_bytes());
            entry_meta.push((e.file_code, e.chunk, path_pos));
        }
        img.flush()?;

        if let Some(last) = chunk_data.last_mut() {
            last.extend_from_slice(b"end\0");
        }

        let mut entries_data = Vec::with_capacity(entry_meta.len() * 8);
        for (fc, chunk, path_pos) in &entry_meta {
            entries_data.extend_from_slice(&fc.to_le_bytes());
            entries_data.extend_from_slice(&chunk.to_le_bytes());
            entries_data.extend_from_slice(&path_pos.to_le_bytes());
        }

        let mut chunk_info = Vec::with_capacity(num_chunks * 12);
        let mut chunk_blob = Vec::new();
        for cd in &chunk_data {
            let comp = zlib_compress(cd);
            chunk_info.extend_from_slice(&(cd.len() as u32).to_le_bytes());
            chunk_info.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            chunk_info.extend_from_slice(&(chunk_blob.len() as u32).to_le_bytes());
            chunk_blob.extend_from_slice(&comp);
        }

        let info_off = 12 + entries_data.len();
        let data_off = info_off + chunk_info.len();
        let mut fl = Vec::with_capacity(data_off + chunk_blob.len());
        fl.extend_from_slice(&(info_off as u32).to_le_bytes());
        fl.extend_from_slice(&(data_off as u32).to_le_bytes());
        fl.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        fl.extend_from_slice(&entries_data);
        fl.extend_from_slice(&chunk_info);
        fl.extend_from_slice(&chunk_blob);
        std::fs::write(out_filelist, fl)?;
        Ok(())
    }
}

/// Same filesystem as `path`, so `fs::rename` into place is atomic.
fn tmp_sibling(path: &Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    // A sequence number keeps concurrent repacks from sharing a temp path.
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!("{name}.tmp{}-{seq}", std::process::id()))
}

/// Unlike seek + read this leaves the file position alone, so parallel workers can share a handle.
#[cfg(unix)]
fn read_exact_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, offset)
}

#[cfg(windows)]
fn read_exact_at(f: &File, buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut buf = buf;
    while !buf.is_empty() {
        match f.seek_read(buf, offset) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "white_img ended before the entry did",
                ));
            }
            Ok(n) => {
                let rest = buf;
                buf = &mut rest[n..];
                offset += n as u64;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data)
        .expect("zlib write to Vec is infallible");
    enc.finish().expect("zlib finish to Vec is infallible")
}

fn parse_record(
    file_code: u32,
    type_id: Option<u8>,
    chunk: u16,
    record: &str,
) -> Result<FileEntry> {
    let mut parts = record.splitn(4, ':');
    let pos = next_hex(&mut parts, record)?;
    let uncmp_size = next_hex(&mut parts, record)?;
    let cmp_size = next_hex(&mut parts, record)?;
    let path = parts.next().unwrap_or("");
    let path = if path == " " {
        String::new()
    } else {
        path.to_string()
    };
    Ok(FileEntry {
        file_code,
        type_id,
        chunk,
        path,
        offset: pos as u64 * SECTOR,
        uncmp_size,
        cmp_size,
    })
}

fn next_hex(parts: &mut std::str::SplitN<char>, record: &str) -> Result<u32> {
    let s = parts
        .next()
        .ok_or_else(|| malformed("filelist", &format!("short record {record:?}")))?;
    u32::from_str_radix(s, 16)
        .map_err(|_| malformed("filelist", &format!("bad hex in record {record:?}")))
}

fn read_cstr(buf: &[u8], pos: usize) -> Result<String> {
    let slice = buf
        .get(pos..)
        .ok_or_else(|| malformed("filelist", "path offset out of range"))?;
    let end = slice
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| malformed("filelist", "unterminated path string"))?;
    Ok(String::from_utf8_lossy(&slice[..end]).into_owned())
}

/// Drops `.`, `..` and empty components so a malformed entry cannot escape the tree. LR stores its
/// zone filelists as `../../../zone/...`, which this clamps to `zone/...`.
fn safe_join(base: &Path, rel: &str) -> std::path::PathBuf {
    let mut p = base.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            continue;
        }
        p.push(comp);
    }
    p
}

fn malformed(format: &'static str, detail: &str) -> FormatError {
    FormatError::Malformed {
        format,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repack_real_subset_if_present() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let full = Filelist::read(format!("{dir}/sys/filelistu.win32.bin"), Game::XIII).unwrap();
        let take = full
            .entries
            .iter()
            .take(20)
            .filter(|e| e.chunk == 0)
            .cloned()
            .collect::<Vec<_>>();
        let subset = Filelist {
            game: Game::XIII,
            encrypted: false,
            entries: take,
        };

        let tmp = std::env::temp_dir().join(format!(
            "ff13_repack_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let ex1 = tmp.join("ex1");
        subset
            .unpack_all(format!("{dir}/sys/white_imgu.win32.bin"), &ex1)
            .unwrap();
        subset
            .repack(&ex1, tmp.join("img2.bin"), tmp.join("fl2.bin"))
            .unwrap();

        let fl2 = Filelist::read(tmp.join("fl2.bin"), Game::XIII).unwrap();
        assert_eq!(fl2.entries.len(), subset.entries.len());
        let ex2 = tmp.join("ex2");
        fl2.unpack_all(tmp.join("img2.bin"), &ex2).unwrap();

        for e in &subset.entries {
            let a = std::fs::read(safe_join(&ex1, &e.path)).unwrap();
            let b = std::fs::read(safe_join(&ex2, &e.path)).unwrap();
            assert_eq!(a, b, "repacked file {} differs", e.path);
        }
        eprintln!("repack round-trip OK for {} files", subset.entries.len());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn unpack_all_parallel_round_trips() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let plain = b"hello world".to_vec();
        let payload = b"the quick brown fox jumps over the lazy dog. ".repeat(8);
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&payload).unwrap();
        let compressed = enc.finish().unwrap();
        let nopath = b"orphan bytes".to_vec();

        let layout = [
            (
                "chr/a.bin",
                0u64,
                &plain,
                plain.len() as u32,
                plain.len() as u32,
            ),
            (
                "chr/b.bin",
                SECTOR,
                &compressed,
                payload.len() as u32,
                compressed.len() as u32,
            ),
            (
                "",
                SECTOR * 2,
                &nopath,
                nopath.len() as u32,
                nopath.len() as u32,
            ),
        ];
        let total = SECTOR * 2 + nopath.len() as u64;
        let mut img = vec![0u8; total as usize];
        for (_, off, stored, _, _) in &layout {
            img[*off as usize..*off as usize + stored.len()].copy_from_slice(stored);
        }

        let fl = Filelist {
            game: Game::XIII,
            encrypted: false,
            entries: layout
                .iter()
                .enumerate()
                .map(|(i, (path, off, _, uncmp, cmp))| FileEntry {
                    file_code: i as u32,
                    type_id: None,
                    chunk: 0,
                    path: (*path).to_string(),
                    offset: *off,
                    uncmp_size: *uncmp,
                    cmp_size: *cmp,
                })
                .collect(),
        };

        let tmp = std::env::temp_dir().join(format!("ff13_unpack_par_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let img_path = tmp.join("white_img.bin");
        std::fs::write(&img_path, &img).unwrap();
        let out = tmp.join("out");

        assert_eq!(fl.unpack_all(&img_path, &out).unwrap(), 3);
        assert_eq!(std::fs::read(out.join("chr/a.bin")).unwrap(), plain);
        assert_eq!(std::fs::read(out.join("chr/b.bin")).unwrap(), payload);
        assert_eq!(std::fs::read(out.join("noPath/FILE_1")).unwrap(), nopath);

        // The unrelated file stands in for the packed archives a revert must keep.
        let keep = out.join("sys/keep.bin");
        std::fs::create_dir_all(keep.parent().unwrap()).unwrap();
        std::fs::write(&keep, b"archive").unwrap();
        assert_eq!(fl.remove_unpacked(&out, None), 3);
        assert!(!out.join("chr/a.bin").exists());
        assert!(!out.join("chr/b.bin").exists());
        assert!(!out.join("noPath/FILE_1").exists());
        assert!(
            keep.is_file(),
            "remove_unpacked must not touch unrelated files"
        );
        assert_eq!(
            fl.remove_unpacked(&out, None),
            0,
            "second pass removes nothing"
        );

        let other = out.join("other_empty");
        std::fs::create_dir_all(&other).unwrap();
        let mut dirs = fl.unpacked_dirs(&out);
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in &dirs {
            let _ = std::fs::remove_dir(d);
        }
        assert!(!out.join("chr").exists(), "empty unpack dir pruned");
        assert!(!out.join("noPath").exists(), "empty noPath dir pruned");
        assert!(other.is_dir(), "non-unpack empty dir is NOT pruned");
        assert!(out.join("sys").is_dir(), "dir with surviving files kept");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn parses_synthetic_game1_filelist() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let record = b"0:10:10:chr/test.bin\0end\0";
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(record).unwrap();
        let chunk = enc.finish().unwrap();

        let total_files = 1u32;
        let entries = {
            let mut v = Vec::new();
            v.extend_from_slice(&0xCAFEu32.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v
        };
        let info_off = 12 + entries.len();
        let data_off = info_off + 12;

        let mut file = Vec::new();
        file.extend_from_slice(&(info_off as u32).to_le_bytes());
        file.extend_from_slice(&(data_off as u32).to_le_bytes());
        file.extend_from_slice(&total_files.to_le_bytes());
        file.extend_from_slice(&entries);
        file.extend_from_slice(&(record.len() as u32).to_le_bytes());
        file.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&chunk);

        let fl = Filelist::parse(&file, Game::XIII).unwrap();
        assert!(!fl.encrypted);
        assert_eq!(fl.entries.len(), 1);
        let e = &fl.entries[0];
        assert_eq!(e.file_code, 0xCAFE);
        assert_eq!(e.path, "chr/test.bin");
        assert_eq!(e.offset, 0);
        assert_eq!(e.uncmp_size, 0x10);
        assert!(!e.is_compressed());
    }
}
