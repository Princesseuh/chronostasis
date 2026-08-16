use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GetClassNameW, GetSystemMetrics,
    GetWindowLongW, SM_CXSCREEN, SM_CYSCREEN, SWP_NOCOPYBITS, SWP_NOSENDCHANGING, SWP_SHOWWINDOW,
    SetWindowLongW, SetWindowPos, ShowCursor, WM_ACTIVATE, WM_ACTIVATEAPP, WNDPROC,
    WS_EX_OVERLAPPEDWINDOW, WS_OVERLAPPEDWINDOW,
};

/// [`crate::patches`] waits on this too: once it exists, `.text` is decrypted and safe to write.
pub(crate) const GAME_WINDOW_CLASS: &str = "SQEX.CDev.Engine.Framework.MainWindow";

static BORDERLESS: AtomicBool = AtomicBool::new(false);
static FORCE_WINDOWED: AtomicBool = AtomicBool::new(false);
static ALWAYS_ACTIVE: AtomicBool = AtomicBool::new(false);
static HIDE_CURSOR: AtomicBool = AtomicBool::new(false);
static TOP_MOST: AtomicBool = AtomicBool::new(false);
static OLD_WNDPROC: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_WINDOW_LONG_W: AtomicUsize = AtomicUsize::new(0);

type CreateWindowExWFn = unsafe extern "system" fn(
    u32,
    *const u16,
    *const u16,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    *mut c_void,
    *mut c_void,
    *const c_void,
) -> HWND;
type CreateWindowExAFn = unsafe extern "system" fn(
    u32,
    *const u8,
    *const u8,
    u32,
    i32,
    i32,
    i32,
    i32,
    HWND,
    *mut c_void,
    *mut c_void,
    *const c_void,
) -> HWND;
type SetWindowLongWFn = unsafe extern "system" fn(HWND, i32, i32) -> i32;

/// Hooks nothing when every option is off.
pub fn configure_and_install(dir: Option<&PathBuf>) {
    crate::patches::for_each_ini_entry(dir, |key, value| match key {
        "borderless" => set_flag(&BORDERLESS, key, value),
        "force_windowed" => set_flag(&FORCE_WINDOWED, key, value),
        "always_active" => set_flag(&ALWAYS_ACTIVE, key, value),
        "hide_cursor" => set_flag(&HIDE_CURSOR, key, value),
        "top_most" => set_flag(&TOP_MOST, key, value),
        _ => {}
    });
    let (borderless, always_active, hide_cursor) = (
        BORDERLESS.load(Ordering::Acquire),
        ALWAYS_ACTIVE.load(Ordering::Acquire),
        HIDE_CURSOR.load(Ordering::Acquire),
    );
    flog!(
        "window: borderless={borderless} force_windowed={} always_active={always_active} hide_cursor={hide_cursor} top_most={}",
        FORCE_WINDOWED.load(Ordering::Acquire),
        TOP_MOST.load(Ordering::Acquire)
    );
    if borderless || always_active || hide_cursor {
        unsafe { install() };
    } else {
        flog!("window: no window options enabled; user32 imports left alone");
    }
}

fn set_flag(cell: &AtomicBool, key: &str, value: &str) {
    let parsed = crate::patches::parse_bool(key, value, cell.load(Ordering::Acquire));
    cell.store(parsed, Ordering::Release);
}

unsafe fn install() {
    unsafe {
        let base = GetModuleHandleW(core::ptr::null()) as usize;
        if base == 0 {
            flog!("window: no game module; user32 hooks skipped");
            return;
        }
        let w = create_window_w as *const c_void;
        let a = create_window_a as *const c_void;
        hook_import(base, "CreateWindowExW", w, &ORIG_CREATE_WINDOW_W);
        hook_import(base, "CreateWindowExA", a, &ORIG_CREATE_WINDOW_A);
        if BORDERLESS.load(Ordering::Acquire) {
            let set_long = set_window_long_w as *const c_void;
            hook_import(base, "SetWindowLongW", set_long, &ORIG_SET_WINDOW_LONG_W);
        }
    }
}

/// Logs the outcome, since the file log is the only diagnostics under Proton.
unsafe fn hook_import(base: usize, func: &str, hook: *const c_void, orig: &AtomicUsize) {
    unsafe {
        match hook_iat(base, "user32.dll", func, hook) {
            Some(o) => {
                orig.store(o, Ordering::Release);
                flog!("window: {func} IAT-hooked (orig={o:#x})");
            }
            None => flog!("window: {func} not hooked (not imported, or the IAT write failed)"),
        }
    }
}

pub(crate) unsafe fn hook_iat(
    base: usize,
    dll: &str,
    func: &str,
    hook: *const c_void,
) -> Option<usize> {
    unsafe {
        let rd32 = |a: usize| core::ptr::read_unaligned(a as *const u32);
        let e_lfanew = rd32(base + 0x3C) as usize;
        let import_rva = rd32(base + e_lfanew + 128) as usize;
        if import_rva == 0 {
            return None;
        }
        let mut desc = base + import_rva;
        loop {
            let name_rva = rd32(desc + 12) as usize;
            if name_rva == 0 {
                return None;
            }
            let dll_name = cstr_ascii(base + name_rva);
            if dll_name.eq_ignore_ascii_case(dll) {
                let oft = rd32(desc) as usize;
                let ft = rd32(desc + 16) as usize;
                let names = if oft != 0 { oft } else { ft };
                let mut i = 0;
                loop {
                    let name_entry = rd32(base + names + i * 4);
                    if name_entry == 0 {
                        break;
                    }
                    if name_entry & 0x8000_0000 == 0 {
                        let fname = cstr_ascii(base + name_entry as usize + 2);
                        if fname == func {
                            let slot = base + ft + i * 4;
                            let original = rd32(slot) as usize;
                            let mut old = 0u32;
                            if VirtualProtect(slot as *mut c_void, 4, PAGE_READWRITE, &mut old) != 0
                            {
                                core::ptr::write_unaligned(slot as *mut u32, hook as usize as u32);
                                let mut tmp = 0u32;
                                VirtualProtect(slot as *mut c_void, 4, old, &mut tmp);
                                return Some(original);
                            }
                            return None;
                        }
                    }
                    i += 1;
                }
            }
            desc += 20;
        }
    }
}

unsafe fn cstr_ascii(mut p: usize) -> String {
    unsafe {
        let mut bytes = Vec::new();
        loop {
            let b = core::ptr::read(p as *const u8);
            if b == 0 {
                break;
            }
            bytes.push(b);
            p += 1;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

unsafe extern "system" fn create_window_w(
    ex: u32,
    cls: *const u16,
    name: *const u16,
    st: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    par: HWND,
    menu: *mut c_void,
    inst: *mut c_void,
    p: *const c_void,
) -> HWND {
    unsafe {
        let orig: CreateWindowExWFn =
            core::mem::transmute(ORIG_CREATE_WINDOW_W.load(Ordering::Acquire));
        let hwnd = orig(ex, cls, name, st, x, y, w, h, par, menu, inst, p);
        on_window_created(hwnd);
        hwnd
    }
}

unsafe extern "system" fn create_window_a(
    ex: u32,
    cls: *const u8,
    name: *const u8,
    st: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    par: HWND,
    menu: *mut c_void,
    inst: *mut c_void,
    p: *const c_void,
) -> HWND {
    unsafe {
        let orig: CreateWindowExAFn =
            core::mem::transmute(ORIG_CREATE_WINDOW_A.load(Ordering::Acquire));
        let hwnd = orig(ex, cls, name, st, x, y, w, h, par, menu, inst, p);
        on_window_created(hwnd);
        hwnd
    }
}

unsafe extern "system" fn set_window_long_w(hwnd: HWND, index: i32, value: i32) -> i32 {
    unsafe {
        let orig: SetWindowLongWFn =
            core::mem::transmute(ORIG_SET_WINDOW_LONG_W.load(Ordering::Acquire));
        let v = if index == GWL_STYLE {
            value & !(WS_OVERLAPPEDWINDOW as i32)
        } else if index == GWL_EXSTYLE {
            value & !(WS_EX_OVERLAPPEDWINDOW as i32)
        } else {
            value
        };
        orig(hwnd, index, v)
    }
}

fn on_window_created(hwnd: HWND) {
    if hwnd.is_null() || !is_game_window(hwnd) {
        return;
    }
    flog!("window: game window created ({:#x})", hwnd as usize);
    if ALWAYS_ACTIVE.load(Ordering::Acquire) || HIDE_CURSOR.load(Ordering::Acquire) {
        unsafe {
            let old = SetWindowLongW(hwnd, GWLP_WNDPROC, wndproc as *const () as usize as i32);
            OLD_WNDPROC.store(old as u32 as usize, Ordering::Release);
            flog!(
                "window: wndproc subclassed (old={:#x})",
                old as u32 as usize
            );
        }
    }
    if BORDERLESS.load(Ordering::Acquire) {
        unsafe { apply_borderless(hwnd) };
        flog!("window: borderless applied");
    }
}

fn is_game_window(hwnd: HWND) -> bool {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == GAME_WINDOW_CLASS
}

unsafe fn apply_borderless(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) & !(WS_OVERLAPPEDWINDOW as i32);
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) & !(WS_EX_OVERLAPPEDWINDOW as i32);
        SetWindowLongW(hwnd, GWL_STYLE, style);
        SetWindowLongW(hwnd, GWL_EXSTYLE, ex);
        fill_screen(hwnd);
    }
}

unsafe fn fill_screen(hwnd: HWND) {
    unsafe {
        let cx = GetSystemMetrics(SM_CXSCREEN);
        let cy = GetSystemMetrics(SM_CYSCREEN);
        let after: HWND = if TOP_MOST.load(Ordering::Acquire) {
            -1isize as *mut c_void
        } else {
            core::ptr::null_mut()
        };
        SetWindowPos(
            hwnd,
            after,
            0,
            0,
            cx,
            cy,
            SWP_SHOWWINDOW | SWP_NOCOPYBITS | SWP_NOSENDCHANGING,
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_ACTIVATE {
            let active = (wparam & 0xFFFF) != 0;
            if active {
                if HIDE_CURSOR.load(Ordering::Acquire) {
                    while ShowCursor(0) >= 0 {}
                }
            } else {
                if ALWAYS_ACTIVE.load(Ordering::Acquire) {
                    return 1;
                }
                if HIDE_CURSOR.load(Ordering::Acquire) {
                    while ShowCursor(1) < 0 {}
                }
            }
        }
        if msg == WM_ACTIVATEAPP && ALWAYS_ACTIVE.load(Ordering::Acquire) {
            return 1;
        }
        let old = OLD_WNDPROC.load(Ordering::Acquire);
        CallWindowProcW(
            core::mem::transmute::<usize, WNDPROC>(old),
            hwnd,
            msg,
            wparam,
            lparam,
        )
    }
}

/// Called from the `CreateDevice` hook.
pub unsafe fn apply_present(pp: *mut c_void) {
    unsafe {
        if pp.is_null() {
            return;
        }
        if FORCE_WINDOWED.load(Ordering::Acquire) {
            core::ptr::write_unaligned((pp as *mut u8).add(32) as *mut u32, 1);
            core::ptr::write_unaligned((pp as *mut u8).add(48) as *mut u32, 0);
        }
        if BORDERLESS.load(Ordering::Acquire) {
            let hwnd = core::ptr::read_unaligned((pp as *const u8).add(28) as *const HWND);
            if !hwnd.is_null() {
                fill_screen(hwnd);
            }
        }
    }
}
