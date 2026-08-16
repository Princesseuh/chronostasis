//! WMP movie container: a flat blob of Bink movies indexed by the `movie_items` WDB.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use byteorder::{BigEndian as BE, ByteOrder};

use crate::{FormatError, Result};

const TABLE_START: usize = 144;

struct MovieEntry {
    name: String,
    group: String,
    record_off: usize,
    wmp_offset: u64,
}

/// e.g. `z000_us.win32.wmp` gives (`z000`, `_us`).
pub fn group_and_vo(wmp_filename: &str) -> (String, String) {
    let vo = if wmp_filename.contains("_us") {
        "_us"
    } else {
        ""
    };
    let group = wmp_filename
        .trim_end_matches(".wmp")
        .replace(".win32", "")
        .replace(".ps3", "")
        .replace(".x360", "")
        .replace("_us", "");
    (group, vo.to_string())
}

fn movie_entries(db: &[u8]) -> Result<Vec<MovieEntry>> {
    let count = BE::read_u32(get(db, 4, 4)?).saturating_sub(4) as usize;
    let string_section = BE::read_u32(get(db, 32, 4)?) as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let base = TABLE_START + i * 32;
        let name = cstr(get(db, base, 16)?);
        let record_ptr = BE::read_u32(get(db, base + 16, 4)?) as usize;
        let group_off = BE::read_u32(get(db, record_ptr, 4)?) as usize;
        let group = cstr(db.get(string_section + group_off..).unwrap_or(&[]));
        let wmp_offset = BE::read_u64(get(db, record_ptr + 8, 8)?);
        entries.push(MovieEntry {
            name,
            group,
            record_off: record_ptr + 4,
            wmp_offset,
        });
    }
    Ok(entries)
}

pub fn unpack(
    db: &[u8],
    wmp_path: &Path,
    wmp_basename: &str,
    vo_suffix: &str,
    out_dir: &Path,
) -> Result<usize> {
    let mut wmp = File::open(wmp_path)?;
    let wmp_len = wmp.metadata()?.len();
    std::fs::create_dir_all(out_dir)?;
    let mut count = 0;
    for e in movie_entries(db)?
        .iter()
        .filter(|e| e.group == wmp_basename)
    {
        // An offset past this file means the movie lives in a different variant.
        if e.wmp_offset + 8 > wmp_len {
            continue;
        }
        // The WDB size can be a stub, so trust the bik header instead.
        let mut hdr = [0u8; 4];
        wmp.seek(SeekFrom::Start(e.wmp_offset + 4))?;
        wmp.read_exact(&mut hdr)?;
        let real_size = u32::from_le_bytes(hdr) as u64 + 8;
        if e.wmp_offset + real_size > wmp_len {
            continue;
        }
        let mut data = vec![0u8; real_size as usize];
        wmp.seek(SeekFrom::Start(e.wmp_offset))?;
        wmp.read_exact(&mut data)?;
        std::fs::write(out_dir.join(format!("{}{vo_suffix}.bik", e.name)), &data)?;
        count += 1;
    }
    Ok(count)
}

/// Patches `db` in place; the caller is responsible for saving it.
pub fn repack(
    db: &mut [u8],
    bik_dir: &Path,
    wmp_basename: &str,
    vo_suffix: &str,
) -> Result<Vec<u8>> {
    let entries = movie_entries(db)?;
    let mut wmp = Vec::new();
    wmp.extend_from_slice(b"WMP\0Ver:0.01\0\0\0\0");
    for e in entries.iter().filter(|e| e.group == wmp_basename) {
        let bik = bik_dir.join(format!("{}{vo_suffix}.bik", e.name));
        if !bik.is_file() {
            continue;
        }
        let data = std::fs::read(&bik)?;
        let offset = wmp.len() as u64;
        wmp.extend_from_slice(&data);
        db[e.record_off..e.record_off + 4].copy_from_slice(&(data.len() as u32).to_be_bytes());
        db[e.record_off + 4..e.record_off + 12].copy_from_slice(&offset.to_be_bytes());
    }
    Ok(wmp)
}

fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn get(buf: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    buf.get(at..at + len).ok_or_else(|| FormatError::Malformed {
        format: "WMP",
        detail: "read past end".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_and_vo_strips_platform_and_vo() {
        assert_eq!(
            group_and_vo("zz099_us.win32.wmp"),
            ("zz099".into(), "_us".into())
        );
        assert_eq!(
            group_and_vo("zz099.win32.wmp"),
            ("zz099".into(), String::new())
        );
        assert_eq!(
            group_and_vo("intro.x360.wmp"),
            ("intro".into(), String::new())
        );
    }

    #[test]
    fn unpack_real_if_present() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let fl = crate::Filelist::read(format!("{dir}/sys/filelistu.win32.bin"), crate::Game::XIII)
            .unwrap();
        let Some(db_entry) = fl
            .entries
            .iter()
            .find(|e| e.path.contains("movie_items") && e.path.ends_with(".wdb"))
        else {
            eprintln!("no movie_items wdb");
            return;
        };
        let mut img = std::fs::File::open(format!("{dir}/sys/white_imgu.win32.bin")).unwrap();
        let db = fl.extract(&mut img, db_entry).unwrap();
        let entries = movie_entries(&db).unwrap();
        eprintln!(
            "movie_items: {} movies; groups: {:?}",
            entries.len(),
            entries
                .iter()
                .map(|e| e.group.as_str())
                .collect::<std::collections::BTreeSet<_>>()
        );

        let wmp = std::path::Path::new(&dir).join("movie/z000.win32.wmp");
        if wmp.is_file() {
            let out = std::env::temp_dir().join(format!("ff13_wmp_{}", std::process::id()));
            let n = unpack(&db, &wmp, "z000", "", &out).unwrap();
            eprintln!("extracted {n} movie(s) from z000.win32.wmp");
            assert!(n > 0, "expected at least one movie in z000.win32.wmp");
            for f in std::fs::read_dir(&out).unwrap().flatten() {
                let head = std::fs::read(f.path()).unwrap();
                let magic = String::from_utf8_lossy(&head[0..3]).into_owned();
                eprintln!("  {} -> magic {magic}", f.file_name().to_string_lossy());
                assert!(
                    magic.starts_with("BIK") || magic.starts_with("KB"),
                    "not Bink"
                );
            }
            std::fs::remove_dir_all(&out).ok();
        }
    }
}
