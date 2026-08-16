//! In-process `d3d9.dll` proxy for FFXIII, applying runtime fixes and memory patches.
//!
//! 32-bit only: `cargo build -p ff13-hooks --release --target i686-pc-windows-gnu`.

#![cfg(windows)]
#![allow(non_snake_case)]

mod device;
mod dinput;
mod font;
mod laa;
#[macro_use]
mod log;
mod patches;
mod proxy;
mod texmod;
mod window;

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{BOOL, HINSTANCE, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// Captured in `DllMain`, and used to locate the DLL's own directory.
pub(crate) static SELF_MODULE: AtomicUsize = AtomicUsize::new(0);

/// A null `module` gives the running exe.
pub(crate) fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut buf = [0u16; 320];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..n])))
}

/// Where `chronostasis.ini` and `chronostasis.log` live.
pub(crate) fn dll_dir() -> Option<PathBuf> {
    let h = SELF_MODULE.load(Ordering::Acquire) as HMODULE;
    module_path(h)?.parent().map(|p| p.to_path_buf())
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        SELF_MODULE.store(hinst as usize, Ordering::Release);
        // The loader lock is held here, so logging has to be buffered.
        log::defer(true);
        flog!("DllMain PROCESS_ATTACH hinst={:#x}", hinst as usize);
        // Must run before the entry point and the DRM bind.
        laa::apply();
        // Lands the init-time patches after `.text` decrypts but before file-system init.
        patches::install_early_hooks();
        log::defer(false);
        std::thread::spawn(patches::run);
    }
    TRUE
}
