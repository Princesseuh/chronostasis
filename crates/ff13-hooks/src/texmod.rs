use core::ffi::c_void;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{OnceLock, RwLock};

use ff13_formats::imgb;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const IUNKNOWN_RELEASE: usize = 2;
const TEX_GET_TYPE: usize = 10;
const TEX_GET_LEVEL_DESC: usize = 17;
const TEX_LOCK_RECT: usize = 19;
const TEX_UNLOCK_RECT: usize = 20;
const D3DRTYPE_TEXTURE: u32 = 3;
const D3DLOCK_READONLY: u32 = 0x10;
const D3DPOOL_MANAGED: u32 = 1;

type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type GetTypeFn = unsafe extern "system" fn(*mut c_void) -> u32;
type GetLevelDescFn = unsafe extern "system" fn(*mut c_void, u32, *mut SurfaceDesc) -> i32;
type LockRectFn =
    unsafe extern "system" fn(*mut c_void, u32, *mut LockedRect, *const c_void, u32) -> i32;
type UnlockRectFn = unsafe extern "system" fn(*mut c_void, u32) -> i32;

#[repr(C)]
#[derive(Default)]
struct SurfaceDesc {
    format: u32,
    rtype: u32,
    usage: u32,
    pool: u32,
    ms_type: u32,
    ms_quality: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct LockedRect {
    pitch: i32,
    bits: *mut c_void,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set when an override targets a header-less texture whose dims cannot be read up front, which
/// forces hashing every bound texture rather than only size matches.
static HASH_ALL: AtomicBool = AtomicBool::new(false);
static MODS_ROOT: OnceLock<PathBuf> = OnceLock::new();
static WHITE_DATA: OnceLock<PathBuf> = OnceLock::new();

static INDEX: RwLock<BTreeMap<u64, PathBuf>> = RwLock::new(BTreeMap::new());
/// Lets the hot path skip locking and hashing any texture that cannot match.
static CAND_DIMS: RwLock<BTreeSet<(u32, u32)>> = RwLock::new(BTreeSet::new());
/// Prebuilt, so the `CreateFileW` hook is a set lookup rather than a disk stat.
static OVERLAY: RwLock<BTreeSet<String>> = RwLock::new(BTreeSet::new());
/// `None` records "hashed, no override", so each texture is hashed only once.
static MAP: RwLock<BTreeMap<usize, Option<usize>>> = RwLock::new(BTreeMap::new());
/// The streaming path opens thousands of files, so logging is capped.
static OVERLAY_LOGGED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Our own reads go through the hooked `CreateFileW` and would otherwise recurse.
    static IN_TEXMOD: Cell<bool> = const { Cell::new(false) };
}

/// Call before any texture loads. `unsafe_gui_overlay` opts GUI containers into the raw file
/// overlay as well; see [`build_indexes`].
pub fn configure(mods_root: PathBuf, unsafe_gui_overlay: bool) {
    let white_data = mods_root.parent().map(|p| p.join("white_data"));
    crate::log::flog!("texmod: mods root = {}", mods_root.display());
    let _ = MODS_ROOT.set(mods_root.clone());
    if let Some(wd) = white_data {
        let _ = WHITE_DATA.set(wd);
    }
    build_indexes(&mods_root, unsafe_gui_overlay);
    ACTIVE.store(true, Ordering::Release);
}

/// Sets the re-entrancy guard, so the resulting opens do not recurse into the redirect.
fn guarded_read(path: &Path) -> Option<Vec<u8>> {
    IN_TEXMOD.with(|f| f.set(true));
    let r = std::fs::read(path).ok();
    IN_TEXMOD.with(|f| f.set(false));
    r
}

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// A loose `.dds` becomes a hash-swap override; everything else becomes a generic overlay, except
/// GUI texture containers, which the swap handles instead.
///
/// `unsafe_gui_overlay` opts those containers into the raw overlay too, which only round-trips for
/// same-dimension swaps, hence off by default.
fn build_indexes(mods_root: &Path, unsafe_gui_overlay: bool) {
    let Some(white_data) = WHITE_DATA.get() else {
        crate::log::flog!("texmod: no white_data root, index empty");
        return;
    };
    let mut index = BTreeMap::new();
    let mut dims = BTreeSet::new();
    let mut overlay = BTreeSet::new();
    for file in walk_all(mods_root) {
        let Ok(rel) = file.strip_prefix(mods_root) else {
            continue;
        };
        let key = rel
            .to_string_lossy()
            .to_ascii_lowercase()
            .replace('/', "\\");
        if key.ends_with(".dds") {
            if let Some((hash, d)) = index_dds(&file, white_data, mods_root) {
                index.insert(hash, file);
                match d {
                    Some(wh) => {
                        dims.insert(wh);
                    }
                    // Dims are unknown here, so pre-filtering by size is not possible.
                    None => HASH_ALL.store(true, Ordering::Release),
                }
            }
            continue;
        }
        // A raw overlay of a GUI container would resize the HUD, so those take the hash swap.
        let is_gui_texture = key.starts_with("gui\\")
            && (key.ends_with(".win32.imgb") || key.ends_with(".win32.xgr"));
        if is_gui_texture && !unsafe_gui_overlay {
            continue;
        }
        overlay.insert(key);
    }
    crate::log::flog!(
        "texmod: indexed {} texture swap(s), {} file overlay(s) (hash_all={})",
        index.len(),
        overlay.len(),
        HASH_ALL.load(Ordering::Acquire)
    );
    if let Ok(mut w) = INDEX.write() {
        *w = index;
    }
    if let Ok(mut w) = CAND_DIMS.write() {
        *w = dims;
    }
    if let Ok(mut w) = OVERLAY.write() {
        *w = overlay;
    }
}

/// `dims` is `None` for the header-less container shape, which has no size to read and so forces
/// hash-all mode.
fn index_dds(dds: &Path, white_data: &Path, mods_root: &Path) -> Option<(u64, Option<(u32, u32)>)> {
    let (ibase, idx) = parse_override(dds)?;
    let rel_dir = dds.parent()?.strip_prefix(mods_root).ok()?;
    let dir = white_data.join(rel_dir);
    let imgb = guarded_read(&dir.join(format!("{ibase}.imgb")))?;
    let header = guarded_read(&dir.join(format!("{ibase}.xgr")))
        .or_else(|| guarded_read(&dir.join(format!("{ibase}.gtex"))));
    match header {
        Some(hdr) => {
            let g = imgb::valid_gtex(&hdr, imgb.len()).into_iter().nth(idx)?;
            let (start, size) = *g.mips.first()?;
            let slice = imgb.get(start as usize..start as usize + size as usize)?;
            Some((
                fnv1a(slice, FNV_OFFSET),
                Some((g.width as u32, g.height as u32)),
            ))
        }
        None => Some((fnv1a(&imgb, FNV_OFFSET), None)),
    }
}

/// Returns `texture` unchanged when there is no override. Hashes mip-0 once, then caches.
///
/// # Safety
/// `device` must be a valid `IDirect3DDevice9` and `texture` a base-texture or null.
pub unsafe fn substitute(device: *mut c_void, texture: *mut c_void) -> *mut c_void {
    unsafe {
        if texture.is_null() || !ACTIVE.load(Ordering::Acquire) {
            return texture;
        }
        let key = texture as usize;
        if let Ok(m) = MAP.read() {
            match m.get(&key) {
                Some(&Some(hd)) => return hd as *mut c_void,
                Some(&None) => return texture,
                None => {}
            }
        }
        let Some(hash) = hash_live(texture) else {
            record(key, None);
            return texture;
        };
        let path = match INDEX.read() {
            Ok(i) => i.get(&hash).cloned(),
            Err(_) => None,
        };
        let Some(path) = path else {
            record(key, None);
            return texture;
        };
        let hd = guarded_read(&path)
            .map(|b| create_texture_from_dds(device, &b))
            .unwrap_or(core::ptr::null_mut());
        if hd.is_null() {
            record(key, None);
            return texture;
        }
        record(key, Some(hd as usize));
        crate::log::flog!(
            "texmod: swapped tex {key:#x} -> HD {:#x} ({})",
            hd as usize,
            path.display()
        );
        hd
    }
}

fn record(key: usize, hd: Option<usize>) {
    if let Ok(mut m) = MAP.write() {
        m.insert(key, hd);
    }
}

/// D3D reuses freed addresses, so a stale entry has to be evicted or a pool of reused objects all
/// show the first one's override.
///
/// # Safety
pub unsafe fn invalidate(texture: *mut c_void) {
    unsafe {
        if texture.is_null() || !ACTIVE.load(Ordering::Acquire) {
            return;
        }
        let key = texture as usize;
        let evicted = MAP.write().ok().and_then(|mut m| m.remove(&key)).flatten();
        if let Some(hd) = evicted {
            let vt = *(hd as *const *const usize);
            let release: ReleaseFn = core::mem::transmute(*vt.add(IUNKNOWN_RELEASE));
            release(hd as *mut c_void);
        }
    }
}

/// `None` unless this is a plain 2D texture in a supported format whose size could match.
unsafe fn hash_live(texture: *mut c_void) -> Option<u64> {
    unsafe {
        let vt = *(texture as *const *const usize);
        let get_type: GetTypeFn = core::mem::transmute(*vt.add(TEX_GET_TYPE));
        if get_type(texture) != D3DRTYPE_TEXTURE {
            return None;
        }
        let get_desc: GetLevelDescFn = core::mem::transmute(*vt.add(TEX_GET_LEVEL_DESC));
        let mut desc = SurfaceDesc::default();
        if get_desc(texture, 0, &mut desc) < 0 {
            return None;
        }
        if !HASH_ALL.load(Ordering::Acquire)
            && !CAND_DIMS
                .read()
                .map(|d| d.contains(&(desc.width, desc.height)))
                .unwrap_or(false)
        {
            return None;
        }
        let (row_pitch, row_count) = row_dims(desc.format, desc.width, desc.height)?;
        let lock: LockRectFn = core::mem::transmute(*vt.add(TEX_LOCK_RECT));
        let unlock: UnlockRectFn = core::mem::transmute(*vt.add(TEX_UNLOCK_RECT));
        let mut lr = LockedRect {
            pitch: 0,
            bits: core::ptr::null_mut(),
        };
        if lock(texture, 0, &mut lr, core::ptr::null(), D3DLOCK_READONLY) < 0 || lr.bits.is_null() {
            return None;
        }
        let mut h = FNV_OFFSET;
        for row in 0..row_count {
            let p = (lr.bits as *const u8).add(row * lr.pitch as usize);
            h = fnv1a(core::slice::from_raw_parts(p, row_pitch), h);
        }
        unlock(texture, 0);
        Some(h)
    }
}

/// `None` for a format this does not handle.
fn row_dims(format: u32, w: u32, h: u32) -> Option<(usize, usize)> {
    let (block, unit) = if format == fourcc(b"DXT1") {
        (true, 8)
    } else if [b"DXT2", b"DXT3", b"DXT4", b"DXT5"]
        .iter()
        .any(|&f| format == fourcc(f))
    {
        (true, 16)
    } else {
        match format {
            21 | 22 => (false, 4),
            28 | 50 => (false, 1),
            23 | 25 | 26 | 51 => (false, 2),
            _ => return None,
        }
    };
    if block {
        Some(((w as usize).div_ceil(4) * unit, (h as usize).div_ceil(4)))
    } else {
        Some((w as usize * unit, h as usize))
    }
}

const fn fourcc(b: &[u8; 4]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Goes through the original `CreateTexture`, to avoid recursing or picking up the mipmap fix.
unsafe fn create_texture_from_dds(device: *mut c_void, dds: &[u8]) -> *mut c_void {
    unsafe {
        if dds.len() < 128 || &dds[0..4] != b"DDS " {
            return core::ptr::null_mut();
        }
        let rd = |o: usize| u32::from_le_bytes([dds[o], dds[o + 1], dds[o + 2], dds[o + 3]]);
        let height = rd(12);
        let width = rd(16);
        let mips = rd(28).max(1);
        let (d3dfmt, block_bytes) = match &dds[84..88] {
            b"DXT1" => (fourcc(b"DXT1"), 8u32),
            b"DXT3" => (fourcc(b"DXT3"), 16),
            b"DXT5" => (fourcc(b"DXT5"), 16),
            other => {
                crate::log::flog!("texmod: unsupported DDS fourcc {other:?}");
                return core::ptr::null_mut();
            }
        };

        let mut tex: *mut c_void = core::ptr::null_mut();
        let hr = crate::device::orig_create_texture(
            device,
            width,
            height,
            mips,
            0,
            d3dfmt,
            D3DPOOL_MANAGED,
            &mut tex,
        );
        if hr < 0 || tex.is_null() {
            crate::log::flog!("texmod: CreateTexture failed hr={hr:#x}");
            return core::ptr::null_mut();
        }

        let vt = *(tex as *const *const usize);
        let lock: LockRectFn = core::mem::transmute(*vt.add(TEX_LOCK_RECT));
        let unlock: UnlockRectFn = core::mem::transmute(*vt.add(TEX_UNLOCK_RECT));
        let body = &dds[128..];
        let mut off = 0usize;
        for level in 0..mips {
            let mw = (width >> level).max(1);
            let mh = (height >> level).max(1);
            let src_pitch = (mw.div_ceil(4).max(1) * block_bytes) as usize;
            let rows = mh.div_ceil(4).max(1) as usize;
            let size = src_pitch * rows;
            if off + size > body.len() {
                crate::log::flog!("texmod: DDS short at mip {level}");
                break;
            }
            let mut lr = LockedRect {
                pitch: 0,
                bits: core::ptr::null_mut(),
            };
            if lock(tex, level, &mut lr, core::ptr::null(), 0) < 0 || lr.bits.is_null() {
                break;
            }
            let dst_pitch = lr.pitch as usize;
            let copy = src_pitch.min(dst_pitch);
            for r in 0..rows {
                core::ptr::copy_nonoverlapping(
                    body.as_ptr().add(off + r * src_pitch),
                    (lr.bits as *mut u8).add(r * dst_pitch),
                    copy,
                );
            }
            unlock(tex, level);
            off += size;
        }
        tex
    }
}

/// Serves the modded copy of any file mirrored under `mods/`. GUI texture containers are excluded
/// at index time, since those take the hash swap above.
///
/// # Safety
/// `name` must be a valid NUL-terminated wide string or null.
pub unsafe fn redirect_open_w(name: *const u16) -> Option<Vec<u16>> {
    unsafe {
        if !ACTIVE.load(Ordering::Acquire) || IN_TEXMOD.with(|f| f.get()) {
            return None;
        }
        let path = wide_to_string(name)?;
        let target = redirect_target(&path)?;
        log_overlay(&target);
        let mut w: Vec<u16> = target.as_os_str().encode_wide().collect();
        w.push(0);
        Some(w)
    }
}

/// ANSI variant of [`redirect_open_w`].
///
/// # Safety
/// `name` must be a valid NUL-terminated byte string or null.
pub unsafe fn redirect_open_a(name: *const u8) -> Option<Vec<u8>> {
    unsafe {
        if !ACTIVE.load(Ordering::Acquire) || IN_TEXMOD.with(|f| f.get()) {
            return None;
        }
        let path = byte_to_string(name)?;
        let target = redirect_target(&path)?;
        log_overlay(&target);
        let mut b: Vec<u8> = target.to_string_lossy().into_owned().into_bytes();
        b.push(0);
        Some(b)
    }
}

/// Only the first few, since streaming pulls thousands of files through here.
fn log_overlay(target: &Path) {
    const MAX_LOGGED: usize = 16;
    match OVERLAY_LOGGED.fetch_add(1, Ordering::Relaxed) {
        n if n < MAX_LOGGED => {
            crate::log::flog!("texmod: file overlay -> {}", target.display())
        }
        n if n == MAX_LOGGED => crate::log::flog!("texmod: further file overlays not logged"),
        _ => {}
    }
}

/// A hit means the file is in the prebuilt [`OVERLAY`] set.
fn redirect_target(path: &str) -> Option<PathBuf> {
    let root = MODS_ROOT.get()?;
    let rel = rel_after_white_data(path)?;
    let key = rel.to_ascii_lowercase().replace('/', "\\");
    if OVERLAY.read().ok()?.contains(&key) {
        Some(root.join(&rel))
    } else {
        None
    }
}

fn walk_all(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Splits `<imgb-base>.<index>.dds`, where the index is the texture's position in the container.
/// No numeric tail means index 0.
fn parse_override(dds: &Path) -> Option<(String, usize)> {
    let name = dds.file_name()?.to_str()?.strip_suffix(".dds")?;
    match name.rsplit_once('.') {
        Some((base, idx)) if !idx.is_empty() && idx.bytes().all(|b| b.is_ascii_digit()) => {
            Some((base.to_string(), idx.parse().ok()?))
        }
        _ => Some((name.to_string(), 0)),
    }
}

/// Preserves the original case.
fn rel_after_white_data(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase().replace('/', "\\");
    let pos = lower.find("white_data\\")?;
    path.get(pos + "white_data\\".len()..)
        .map(|s| s.to_string())
}

unsafe fn wide_to_string(p: *const u16) -> Option<String> {
    unsafe {
        if p.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
            if len > 0x7FFF {
                return None;
            }
        }
        Some(String::from_utf16_lossy(core::slice::from_raw_parts(
            p, len,
        )))
    }
}

unsafe fn byte_to_string(p: *const u8) -> Option<String> {
    unsafe {
        if p.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
            if len > 0x7FFF {
                return None;
            }
        }
        Some(String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).into_owned())
    }
}
