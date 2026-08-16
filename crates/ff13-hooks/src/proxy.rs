use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryA;

static REAL: AtomicUsize = AtomicUsize::new(0);

fn real_d3d9() -> HMODULE {
    let cached = REAL.load(Ordering::Acquire);
    if cached != 0 {
        return cached as HMODULE;
    }
    let h = unsafe { load_real() };
    REAL.store(h as usize, Ordering::Release);
    h
}

unsafe fn load_real() -> HMODULE {
    unsafe {
        // Under the `d3d9=n,b` override Wine matches "d3d9.dll" by basename back to us, and forwarding
        // to self recurses forever.
        let self_mod = crate::SELF_MODULE.load(Ordering::Acquire) as HMODULE;

        // The Proton path, where the builtin d3d9 is shadowed by us.
        let h = LoadLibraryA(c"dxvk.dll".as_ptr() as *const u8);
        flog!(
            "load_real: dxvk.dll -> {:#x} (self={:#x})",
            h as usize,
            self_mod as usize
        );
        if !h.is_null() && h != self_mod {
            return h;
        }
        // Real d3d9 on native Windows; under Wine this resolves back to us and is rejected above.
        let mut buf = [0u8; 260];
        let n = GetSystemDirectoryA(buf.as_mut_ptr(), buf.len() as u32) as usize;
        let mut path: Vec<u8> = Vec::with_capacity(n + 12);
        path.extend_from_slice(&buf[..n]);
        path.extend_from_slice(b"\\d3d9.dll\0");
        let h = LoadLibraryA(path.as_ptr());
        flog!("load_real: system d3d9.dll -> {:#x}", h as usize);
        if h == self_mod {
            flog!("load_real: system d3d9 resolved back to self; no real d3d9 available!");
            core::ptr::null_mut()
        } else {
            h
        }
    }
}

unsafe fn resolve(name: &[u8]) -> *const c_void {
    unsafe {
        let h = real_d3d9();
        if h.is_null() {
            return core::ptr::null();
        }
        match GetProcAddress(h, name.as_ptr()) {
            Some(f) => f as usize as *const c_void,
            None => core::ptr::null(),
        }
    }
}

macro_rules! forward {
    ($name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty, $sym:literal, $default:expr_2021) => {
        #[unsafe(no_mangle)]
        #[allow(clippy::unused_unit)]
        pub unsafe extern "system" fn $name($($arg: $ty),*) -> $ret { unsafe {
            let p = resolve($sym);
            if p.is_null() {
                return $default;
            }
            let f: extern "system" fn($($ty),*) -> $ret = core::mem::transmute(p);
            f($($arg),*)
        }}
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Direct3DCreate9(sdk: u32) -> *mut c_void {
    unsafe {
        flog!("Direct3DCreate9 enter");
        let p = resolve(b"Direct3DCreate9\0");
        if p.is_null() {
            flog!("Direct3DCreate9: resolve failed (no real d3d9) -> returning null");
            return core::ptr::null_mut();
        }
        let f: extern "system" fn(u32) -> *mut c_void = core::mem::transmute(p);
        let d3d9 = f(sdk);
        flog!("Direct3DCreate9: real returned {:#x}", d3d9 as usize);
        crate::device::hook_d3d9(d3d9);
        flog!("Direct3DCreate9: hook_d3d9 done");
        d3d9
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Direct3DCreate9Ex(sdk: u32, out: *mut *mut c_void) -> i32 {
    unsafe {
        flog!("Direct3DCreate9Ex enter");
        let p = resolve(b"Direct3DCreate9Ex\0");
        if p.is_null() {
            flog!("Direct3DCreate9Ex: resolve failed (no real d3d9) -> returning -1");
            return -1;
        }
        let f: extern "system" fn(u32, *mut *mut c_void) -> i32 = core::mem::transmute(p);
        let hr = f(sdk, out);
        flog!("Direct3DCreate9Ex: real hr={hr:#x}");
        if hr >= 0 && !out.is_null() {
            crate::device::hook_d3d9(*out);
            flog!("Direct3DCreate9Ex: hook_d3d9 done");
        }
        hr
    }
}

forward!(D3DPERF_BeginEvent(col: u32, name: *const u16) -> i32, b"D3DPERF_BeginEvent\0", 0);
forward!(D3DPERF_EndEvent() -> i32, b"D3DPERF_EndEvent\0", 0);
forward!(D3DPERF_GetStatus() -> u32, b"D3DPERF_GetStatus\0", 0);
forward!(D3DPERF_QueryRepeatFrame() -> i32, b"D3DPERF_QueryRepeatFrame\0", 0);
forward!(D3DPERF_SetMarker(col: u32, name: *const u16) -> (), b"D3DPERF_SetMarker\0", ());
forward!(D3DPERF_SetOptions(opts: u32) -> (), b"D3DPERF_SetOptions\0", ());
forward!(D3DPERF_SetRegion(col: u32, name: *const u16) -> (), b"D3DPERF_SetRegion\0", ());
