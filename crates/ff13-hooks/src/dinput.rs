use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::log::flog;

const E_FAIL: i32 = 0x8000_4005u32 as i32;

const DI8_CREATE_DEVICE: usize = 3;
const DEV_GET_DEVICE_STATE: usize = 9;
const DEV_GET_DEVICE_INFO: usize = 15;

// `lZ` and `lRz` are the trigger axes; `lRz` folds into `lZ`.
const DIJOYSTATE_SIZE: u32 = 0x50;
const DIJOYSTATE_LZ: usize = 0x08;
const DIJOYSTATE_LRZ: usize = 0x14;

// The ANSI and Unicode forms differ in both character width and struct size.
const DIDEVICEINSTANCEA_SIZE: u32 = 0x244;
const DIDEVICEINSTANCEW_SIZE: u32 = 0x44C;
const PRODUCT_NAME_OFF_A: usize = 0x12C;
const PRODUCT_NAME_OFF_W: usize = 0x230;
const MAX_PATH_CHARS: usize = 260;

// Neutral is 128, with LT driving toward 255 and RT toward 0.
const AXIS_RANGE: f64 = 255.0;
const LEFT_SCALE: f64 = 127.0;
const RIGHT_SCALE: f64 = 128.0;
const NEUTRAL: f64 = 128.0;

type DirectInput8CreateFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    *const c_void,
    *mut *mut c_void,
    *mut c_void,
) -> i32;
type CreateDeviceFn =
    unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
type GetDeviceStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
type GetDeviceInfoFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;

static ORIG_DIRECTINPUT8CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_DEVICE_STATE: AtomicUsize = AtomicUsize::new(0);

static CREATE_DEVICE_HOOKED: AtomicBool = AtomicBool::new(false);
static GET_STATE_HOOKED: AtomicBool = AtomicBool::new(false);
static LOGGED_RECOMBINE: AtomicBool = AtomicBool::new(false);
// Affects only how `GetDeviceInfo` reads the product-name string.
static WIDE: AtomicBool = AtomicBool::new(false);

// The device vtable is shared across keyboard, mouse and joysticks alike, so the hook fires for
// everything; this marks which devices to actually remap.
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY: AtomicUsize = AtomicUsize::new(0);
static XBONE_DEVICES: [AtomicUsize; 8] = [EMPTY; 8];

/// Must run before any DirectInput device is created.
pub fn install(enabled: bool) {
    if !enabled {
        return;
    }
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    if base == 0 {
        return;
    }
    let hook = directinput8create_hook as *const c_void;
    match unsafe { crate::window::hook_iat(base, "dinput8.dll", "DirectInput8Create", hook) } {
        Some(orig) => {
            ORIG_DIRECTINPUT8CREATE.store(orig, Ordering::Release);
            flog!("dinput: DirectInput8Create IAT-hooked (orig={orig:#x})");
        }
        // No static import means the game either does not use DI8 or links it dynamically.
        None => flog!("dinput: DirectInput8Create not in game IAT; trigger fix inactive"),
    }
}

unsafe extern "system" fn directinput8create_hook(
    hinst: *mut c_void,
    version: u32,
    riid: *const c_void,
    ppv: *mut *mut c_void,
    punk: *mut c_void,
) -> i32 {
    unsafe {
        let orig: DirectInput8CreateFn =
            core::mem::transmute(ORIG_DIRECTINPUT8CREATE.load(Ordering::Acquire));
        let hr = orig(hinst, version, riid, ppv, punk);
        if hr < 0 || ppv.is_null() {
            return hr;
        }
        WIDE.store(
            !riid.is_null() && *(riid as *const u8) == 0x31,
            Ordering::Release,
        );
        let di8 = *ppv;
        if !di8.is_null() && !CREATE_DEVICE_HOOKED.swap(true, Ordering::AcqRel) {
            crate::device::hook_slot_store(
                di8,
                DI8_CREATE_DEVICE,
                create_device_hook as *const c_void,
                &ORIG_CREATE_DEVICE,
            );
            flog!(
                "dinput: IDirectInput8::CreateDevice hooked (wide={})",
                WIDE.load(Ordering::Acquire)
            );
        }
        hr
    }
}

unsafe extern "system" fn create_device_hook(
    this: *mut c_void,
    rguid: *const c_void,
    out: *mut *mut c_void,
    punk: *mut c_void,
) -> i32 {
    unsafe {
        let orig: CreateDeviceFn = core::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Acquire));
        let hr = orig(this, rguid, out, punk);
        if hr < 0 || out.is_null() {
            return hr;
        }
        let dev = *out;
        if dev.is_null() || !is_xbox_one_device(dev) {
            return hr;
        }
        flog!(
            "dinput: Xbox One pad detected ({:#x}); applying trigger fix",
            dev as usize
        );
        record_device(dev as usize);
        if !GET_STATE_HOOKED.swap(true, Ordering::AcqRel) {
            crate::device::hook_slot_store(
                dev,
                DEV_GET_DEVICE_STATE,
                get_device_state_hook as *const c_void,
                &ORIG_GET_DEVICE_STATE,
            );
            flog!(
                "dinput: GetDeviceState hooked (orig={:#x})",
                ORIG_GET_DEVICE_STATE.load(Ordering::Acquire)
            );
        }
        hr
    }
}

unsafe extern "system" fn get_device_state_hook(
    this: *mut c_void,
    cb: u32,
    data: *mut c_void,
) -> i32 {
    unsafe {
        let o = ORIG_GET_DEVICE_STATE.load(Ordering::Acquire);
        if o == 0 {
            return E_FAIL;
        }
        let orig: GetDeviceStateFn = core::mem::transmute(o);
        let hr = orig(this, cb, data);
        // The shared vtable fires for keyboard and mouse too, so both checks are needed.
        if hr >= 0 && cb == DIJOYSTATE_SIZE && !data.is_null() && is_recorded(this as usize) {
            let lz =
                core::ptr::read_unaligned((data as *const u8).add(DIJOYSTATE_LZ) as *const i32);
            let lrz =
                core::ptr::read_unaligned((data as *const u8).add(DIJOYSTATE_LRZ) as *const i32);
            let combined = (lz as f64 / AXIS_RANGE) * LEFT_SCALE + NEUTRAL
                - (lrz as f64 / AXIS_RANGE) * RIGHT_SCALE;
            let z = (combined as i32) & 0xFF;
            core::ptr::write_unaligned((data as *mut u8).add(DIJOYSTATE_LZ) as *mut i32, z);
            if !LOGGED_RECOMBINE.swap(true, Ordering::AcqRel) {
                flog!("dinput: trigger recombine active (lz={lz} lrz={lrz} -> z={z})");
            }
        }
        hr
    }
}

/// Matches on an "xbox one" substring rather than a full name, so the various One-pad strings all
/// match while the 360 pad does not.
unsafe fn is_xbox_one_device(dev: *mut c_void) -> bool {
    unsafe {
        let wide = WIDE.load(Ordering::Acquire);
        let vtable = *(dev as *const *const usize);
        let get_info: GetDeviceInfoFn = core::mem::transmute(*vtable.add(DEV_GET_DEVICE_INFO));

        let mut buf = [0u8; 0x500];
        let size = if wide {
            DIDEVICEINSTANCEW_SIZE
        } else {
            DIDEVICEINSTANCEA_SIZE
        };
        core::ptr::write_unaligned(buf.as_mut_ptr() as *mut u32, size);
        if get_info(dev, buf.as_mut_ptr() as *mut c_void) < 0 {
            return false;
        }

        let name = if wide {
            let p = buf.as_ptr().add(PRODUCT_NAME_OFF_W) as *const u16;
            let mut chars = Vec::new();
            for i in 0..MAX_PATH_CHARS {
                let c = core::ptr::read_unaligned(p.add(i));
                if c == 0 {
                    break;
                }
                chars.push(c);
            }
            String::from_utf16_lossy(&chars)
        } else {
            let p = buf.as_ptr().add(PRODUCT_NAME_OFF_A);
            let mut bytes = Vec::new();
            for i in 0..MAX_PATH_CHARS {
                let c = *p.add(i);
                if c == 0 {
                    break;
                }
                bytes.push(c);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        };
        flog!("dinput: device product name = {name:?}");
        name.to_ascii_lowercase().contains("xbox one")
    }
}

/// Idempotent.
fn record_device(ptr: usize) {
    if is_recorded(ptr) {
        return;
    }
    for slot in &XBONE_DEVICES {
        if slot
            .compare_exchange(0, ptr, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

fn is_recorded(ptr: usize) -> bool {
    XBONE_DEVICES
        .iter()
        .any(|s| s.load(Ordering::Acquire) == ptr)
}
