use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesA, GetFileAttributesW};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

const LAA_FLAG: u8 = 0x20;
const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;

static ORIG_CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_HOOKED: AtomicBool = AtomicBool::new(false);
/// The redirect is one-shot, because SteamStub re-reads the exe and a second redirect would
/// corrupt it. The hook itself stays installed for the session, since LayeredFS needs it.
static REDIRECT_DONE: AtomicBool = AtomicBool::new(false);
/// Armed only by [`apply`], so with no pristine copy the per-open scan is skipped entirely.
static REDIRECT_ARMED: AtomicBool = AtomicBool::new(false);
static TEXMOD_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Idempotent, and shares the hook with the self-read redirect if that installed it first.
pub fn enable_texture_mods() {
    TEXMOD_ACTIVE.store(true, Ordering::Release);
    unsafe { ensure_create_file_hooked() };
}

type CreateFileWFn = unsafe extern "system" fn(
    *const u16,
    u32,
    u32,
    *const c_void,
    u32,
    u32,
    *mut c_void,
) -> *mut c_void;
type CreateFileAFn = unsafe extern "system" fn(
    *const u8,
    u32,
    u32,
    *const c_void,
    u32,
    u32,
    *mut c_void,
) -> *mut c_void;

/// Must run in `DllMain`, before the entry point and the DRM bind.
///
/// FFXIII only: XIII-2 and LR are not SteamStub-protected, boot fine from a plain flip, and would
/// have their headers wrongly cleared by this.
pub fn apply() {
    unsafe {
        if !running_exe_is("ffxiiiimg.exe") {
            return;
        }
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 || *(base as *const u16) != 0x5A4D {
            return;
        }
        let e_lfanew = *((base + 0x3C) as *const u32) as usize;
        if *((base + e_lfanew) as *const u32) != 0x0000_4550 {
            return;
        }
        let flag = base + e_lfanew + 22;
        let checksum = base + e_lfanew + 88;

        if *(flag as *const u8) & LAA_FLAG == 0 || !untouched_exists() {
            return;
        }
        crate::log::flog!("laa: patched exe detected; restoring header + redirecting self-read");

        write_mem(flag, &[*(flag as *const u8) & !LAA_FLAG]);
        write_mem(checksum, &0u32.to_le_bytes());

        REDIRECT_ARMED.store(true, Ordering::Release);
        ensure_create_file_hooked();
    }
}

/// Inline rather than IAT hooking, because SteamStub resolves the API dynamically and would slip
/// past an IAT hook. Must run in `DllMain`, before the integrity check. Idempotent.
unsafe fn ensure_create_file_hooked() {
    unsafe {
        use minhook::MinHook;
        if CREATE_FILE_HOOKED.swap(true, Ordering::AcqRel) {
            return;
        }
        match MinHook::create_hook_api("kernel32.dll", "CreateFileW", create_file_w as _) {
            Ok(t) => ORIG_CREATE_FILE_W.store(t as usize, Ordering::Release),
            Err(e) => crate::log::flog!("laa: CreateFileW hook failed: {e:?}"),
        }
        match MinHook::create_hook_api("kernel32.dll", "CreateFileA", create_file_a as _) {
            Ok(t) => ORIG_CREATE_FILE_A.store(t as usize, Ordering::Release),
            Err(e) => crate::log::flog!("laa: CreateFileA hook failed: {e:?}"),
        }
        match MinHook::enable_all_hooks() {
            Ok(()) => crate::log::flog!(
                "laa: createfile hooks enabled (W tramp={:#x}, A tramp={:#x})",
                ORIG_CREATE_FILE_W.load(Ordering::Acquire),
                ORIG_CREATE_FILE_A.load(Ordering::Acquire)
            ),
            Err(e) => crate::log::flog!("laa: enable_all_hooks failed: {e:?}"),
        }
    }
}

unsafe fn write_mem(addr: usize, bytes: &[u8]) {
    unsafe {
        let mut old = 0u32;
        if VirtualProtect(addr as *mut c_void, bytes.len(), PAGE_READWRITE, &mut old) != 0 {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
            let mut tmp = 0u32;
            VirtualProtect(addr as *mut c_void, bytes.len(), old, &mut tmp);
        }
    }
}

/// Case-insensitive ASCII.
fn running_exe_is(name: &str) -> bool {
    crate::module_path(core::ptr::null_mut()).is_some_and(|p| {
        p.file_name()
            .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name))
    })
}

/// Its presence is what marks the exe as LAA-patched on disk.
unsafe fn untouched_exists() -> bool {
    unsafe {
        let Some(dir) = crate::dll_dir() else {
            return false;
        };
        let mut path: Vec<u16> = dir
            .join("untouched.exe")
            .as_os_str()
            .encode_wide()
            .collect();
        path.push(0);
        GetFileAttributesW(path.as_ptr()) != INVALID_FILE_ATTRIBUTES
    }
}

unsafe extern "system" fn create_file_w(
    name: *const u16,
    access: u32,
    share: u32,
    sec: *const c_void,
    disp: u32,
    flags: u32,
    template: *mut c_void,
) -> *mut c_void {
    unsafe {
        let orig: CreateFileWFn = core::mem::transmute(ORIG_CREATE_FILE_W.load(Ordering::Acquire));
        if REDIRECT_ARMED.load(Ordering::Acquire)
            && !REDIRECT_DONE.load(Ordering::Acquire)
            && let Some(redir) = redirect_w(name)
        {
            REDIRECT_DONE.store(true, Ordering::Release);
            return orig(redir.as_ptr(), access, share, sec, disp, flags, template);
        }
        if TEXMOD_ACTIVE.load(Ordering::Acquire)
            && let Some(redir) = crate::texmod::redirect_open_w(name)
        {
            return orig(redir.as_ptr(), access, share, sec, disp, flags, template);
        }
        orig(name, access, share, sec, disp, flags, template)
    }
}

unsafe extern "system" fn create_file_a(
    name: *const u8,
    access: u32,
    share: u32,
    sec: *const c_void,
    disp: u32,
    flags: u32,
    template: *mut c_void,
) -> *mut c_void {
    unsafe {
        let orig: CreateFileAFn = core::mem::transmute(ORIG_CREATE_FILE_A.load(Ordering::Acquire));
        if REDIRECT_ARMED.load(Ordering::Acquire)
            && !REDIRECT_DONE.load(Ordering::Acquire)
            && let Some(redir) = redirect_a(name)
        {
            REDIRECT_DONE.store(true, Ordering::Release);
            return orig(redir.as_ptr(), access, share, sec, disp, flags, template);
        }
        if TEXMOD_ACTIVE.load(Ordering::Acquire)
            && let Some(redir) = crate::texmod::redirect_open_a(name)
        {
            return orig(redir.as_ptr(), access, share, sec, disp, flags, template);
        }
        orig(name, access, share, sec, disp, flags, template)
    }
}

const PATCHED_EXE: &[u8] = b"ffxiiiimg.exe";
const PRISTINE_EXE: &[u8] = b"untouched.exe";

/// The two names are the same length, so the swap is in place.
unsafe fn redirect_w(name: *const u16) -> Option<Vec<u16>> {
    unsafe {
        let s = wide_slice(name)?;
        let pos = find_ascii(s, PATCHED_EXE)?;
        let mut out = s.to_vec();
        for (dst, &src) in out[pos..].iter_mut().zip(PRISTINE_EXE) {
            *dst = src as u16;
        }
        out.push(0);
        (GetFileAttributesW(out.as_ptr()) != INVALID_FILE_ATTRIBUTES).then_some(out)
    }
}

unsafe fn redirect_a(name: *const u8) -> Option<Vec<u8>> {
    unsafe {
        let s = byte_slice(name)?;
        let pos = find_ascii(s, PATCHED_EXE)?;
        let mut out = s.to_vec();
        out[pos..pos + PRISTINE_EXE.len()].copy_from_slice(PRISTINE_EXE);
        out.push(0);
        (GetFileAttributesA(out.as_ptr()) != INVALID_FILE_ATTRIBUTES).then_some(out)
    }
}

/// Allocation-free, for the per-open path. `needle` must already be lowercase.
fn find_ascii<T: Copy + Into<u32>>(hay: &[T], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| {
        hay[i..i + needle.len()].iter().zip(needle).all(|(&c, &n)| {
            let c: u32 = c.into();
            c < 128 && (c as u8).eq_ignore_ascii_case(&n)
        })
    })
}

unsafe fn wide_slice<'a>(p: *const u16) -> Option<&'a [u16]> {
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
        Some(core::slice::from_raw_parts(p, len))
    }
}

unsafe fn byte_slice<'a>(p: *const u8) -> Option<&'a [u8]> {
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
        Some(core::slice::from_raw_parts(p, len))
    }
}
