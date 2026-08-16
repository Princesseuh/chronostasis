//! Large Address Aware (LAA) patch for the FFXIII-trilogy executables.
//!
//! FFXIII's exe self-integrity-checks, so its bit-flip only works paired with the
//! `ff13-hooks` DLL redirecting that read at the pristine `untouched.exe` kept here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const LAA_FLAG: u8 = 0x20;
const E_LFANEW_OFFSET: usize = 0x3C;
/// Relative to the PE signature: 4 signature bytes, then 18 into the 20-byte header.
const CHARACTERISTICS_REL: usize = 22;

/// The name length matters: the redirect overwrites the 9-character stem in place.
pub const UNTOUCHED_NAME: &str = "untouched.exe";

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("not a valid PE file: {0}")]
    InvalidPe(&'static str),
    #[error(
        "executable is already patched but no pristine `untouched.exe` exists; restore the original first"
    )]
    AlreadyPatchedNoBackup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOutcome {
    Patched,
    AlreadyPatched,
}

fn characteristics_offset(bytes: &[u8]) -> Result<usize, PatchError> {
    if bytes.len() < E_LFANEW_OFFSET + 4 {
        return Err(PatchError::InvalidPe("file too small for DOS header"));
    }
    if &bytes[0..2] != b"MZ" {
        return Err(PatchError::InvalidPe("missing MZ signature"));
    }
    let e_lfanew = u32::from_le_bytes(
        bytes[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    if bytes.get(e_lfanew..e_lfanew + 4) != Some(b"PE\0\0") {
        return Err(PatchError::InvalidPe("missing PE signature"));
    }
    let off = e_lfanew + CHARACTERISTICS_REL;
    if bytes.len() <= off {
        return Err(PatchError::InvalidPe("file too small for COFF header"));
    }
    Ok(off)
}

pub fn is_laa(bytes: &[u8]) -> Result<bool, PatchError> {
    let off = characteristics_offset(bytes)?;
    Ok(bytes[off] & LAA_FLAG != 0)
}

/// Returns whether the buffer changed.
pub fn set_laa(bytes: &mut [u8], enabled: bool) -> Result<bool, PatchError> {
    let off = characteristics_offset(bytes)?;
    let currently = bytes[off] & LAA_FLAG != 0;
    if currently == enabled {
        return Ok(false);
    }
    if enabled {
        bytes[off] |= LAA_FLAG;
    } else {
        bytes[off] &= !LAA_FLAG;
    }
    Ok(true)
}

pub fn is_large_address_aware(exe: &Path) -> Result<bool, PatchError> {
    is_laa(&fs::read(exe)?)
}

/// Copies only when `exe` is currently unpatched, so the copy stays pristine.
fn ensure_untouched_copy(exe: &Path, exe_is_patched: bool) -> Result<PathBuf, PatchError> {
    let dir = exe
        .parent()
        .ok_or(PatchError::InvalidPe("executable has no parent directory"))?;
    let untouched = dir.join(UNTOUCHED_NAME);
    if untouched.exists() {
        return Ok(untouched);
    }
    if exe_is_patched {
        return Err(PatchError::AlreadyPatchedNoBackup);
    }
    fs::copy(exe, &untouched)?;
    Ok(untouched)
}

/// Idempotent. `preserve_untouched` additionally keeps a pristine copy for the runtime self-read
/// redirect, which only the SteamStub-wrapped FFXIII needs.
pub fn apply_laa_patch(exe: &Path, preserve_untouched: bool) -> Result<PatchOutcome, PatchError> {
    let mut bytes = fs::read(exe)?;
    let already = is_laa(&bytes)?;
    if preserve_untouched {
        ensure_untouched_copy(exe, already)?;
    }
    if already {
        return Ok(PatchOutcome::AlreadyPatched);
    }
    set_laa(&mut bytes, true)?;
    fs::write(exe, &bytes)?;
    Ok(PatchOutcome::Patched)
}

pub fn revert_laa_patch(exe: &Path) -> Result<bool, PatchError> {
    let mut bytes = fs::read(exe)?;
    if !set_laa(&mut bytes, false)? {
        return Ok(false);
    }
    fs::write(exe, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_pe() -> Vec<u8> {
        let pe_off: usize = 0x80;
        let mut bytes = vec![0u8; pe_off + 24];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&(pe_off as u32).to_le_bytes());
        bytes[pe_off..pe_off + 4].copy_from_slice(b"PE\0\0");
        bytes
    }

    #[test]
    fn detects_and_sets_flag() {
        let mut pe = synthetic_pe();
        assert!(!is_laa(&pe).unwrap());
        assert!(set_laa(&mut pe, true).unwrap());
        assert!(is_laa(&pe).unwrap());
        assert!(!set_laa(&mut pe, true).unwrap());
        assert!(set_laa(&mut pe, false).unwrap());
        assert!(!is_laa(&pe).unwrap());
    }

    #[test]
    fn preserves_other_characteristic_bits() {
        let mut pe = synthetic_pe();
        let off = characteristics_offset(&pe).unwrap();
        pe[off] = 0x0F;
        set_laa(&mut pe, true).unwrap();
        assert_eq!(pe[off], 0x0F | LAA_FLAG);
        set_laa(&mut pe, false).unwrap();
        assert_eq!(pe[off], 0x0F);
    }

    #[test]
    fn rejects_non_pe() {
        assert!(matches!(is_laa(&[0u8; 4]), Err(PatchError::InvalidPe(_))));
        let mut junk = vec![0u8; 0x100];
        junk[0] = b'M';
        junk[1] = b'Z';
        assert!(matches!(is_laa(&junk), Err(PatchError::InvalidPe(_))));
    }
}
