use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const IDIRECT3D9_CREATE_DEVICE: usize = 16;
const DEV_CREATE_TEXTURE: usize = 23;
const DEV_CREATE_VERTEX_BUFFER: usize = 26;
const DEV_END_SCENE: usize = 42;
const DEV_CLEAR: usize = 43;
const DEV_SET_TEXTURE: usize = 65;
const DEV_SET_SAMPLER_STATE: usize = 69;
const DEV_SET_SCISSOR_RECT: usize = 75;
const DEV_DRAW_PRIMITIVE_UP: usize = 83;
const TEX_SET_AUTOGEN_FILTER: usize = 14;
const TEX_GENERATE_MIPS: usize = 16;

const D3DPOOL_MANAGED: u32 = 1;
const D3DPOOL_SYSTEMMEM: u32 = 2;
const D3DUSAGE_DYNAMIC: u32 = 0x200;
const D3DUSAGE_WRITEONLY: u32 = 0x8;
const D3DUSAGE_AUTOGENMIPMAP: u32 = 0x400;
const D3DPT_TRIANGLEFAN: u32 = 6;
const D3DCLEAR_TARGET: u32 = 0x1;
const OVERLAY_COLOR: u32 = 0xFFFF_00FF;
const OVERLAY_BG: u32 = 0xFF10_1018;
const OVERLAY_SCALE: i32 = 2;
const OVERLAY_SECS: u64 = 3;
// MAXANISOTROPY is 10, not 9; picking 9 pins every texture to its smallest mip.
const D3DSAMP_MINFILTER: u32 = 6;
const D3DSAMP_MIPFILTER: u32 = 7;
const D3DSAMP_MIPMAPLODBIAS: u32 = 8;
const D3DSAMP_MAXANISOTROPY: u32 = 10;
const D3DTEXF_LINEAR: u32 = 2;
const D3DTEXF_ANISOTROPIC: u32 = 3;
/// Both games lock a 358400-byte vertex buffer before drawing any 2D element.
const UI_VERTEX_BUFFER_LEN: u32 = 358400;

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

type CreateDeviceFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    u32,
    *mut c_void,
    u32,
    *mut c_void,
    *mut *mut c_void,
) -> i32;
type SetScissorRectFn = unsafe extern "system" fn(*mut c_void, *const Rect) -> i32;
type CreateVertexBufferFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    u32,
    *mut *mut c_void,
    *mut *mut c_void,
) -> i32;
type DrawPrimitiveUpFn =
    unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void, u32) -> i32;
type SetSamplerStateFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32) -> i32;
type CreateTextureFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    *mut *mut c_void,
    *mut c_void,
) -> i32;
type SetTextureFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
type SetAutoGenFilterFn = unsafe extern "system" fn(*mut c_void, u32) -> i32;
type GenerateMipsFn = unsafe extern "system" fn(*mut c_void);
type EndSceneFn = unsafe extern "system" fn(*mut c_void) -> i32;
type ClearFn = unsafe extern "system" fn(*mut c_void, u32, *const Rect, u32, u32, f32, u32) -> i32;

static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_SCISSOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_VB: AtomicUsize = AtomicUsize::new(0);
static ORIG_DRAW_UP: AtomicUsize = AtomicUsize::new(0);

static D3D9_HOOKED: AtomicBool = AtomicBool::new(false);
static DEVICE_HOOKED: AtomicBool = AtomicBool::new(false);

static LOGGED_SCISSOR: AtomicBool = AtomicBool::new(false);
static LOGGED_VB: AtomicBool = AtomicBool::new(false);
static LOGGED_DRAW: AtomicBool = AtomicBool::new(false);

static DEVICE_FIXES: AtomicBool = AtomicBool::new(true);

/// Set before device creation.
pub fn set_device_fixes(on: bool) {
    DEVICE_FIXES.store(on, Ordering::Release);
}

// 0 leaves the game's choice; the address is of its internal-res global, width then height.
static RENDER_W: AtomicU32 = AtomicU32::new(0);
static RENDER_H: AtomicU32 = AtomicU32::new(0);
static RENDER_RES_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Past the launcher's cap. Set before device creation.
pub fn set_render_override(internal_res_addr: usize, w: u32, h: u32) {
    RENDER_RES_ADDR.store(internal_res_addr, Ordering::Release);
    RENDER_W.store(w, Ordering::Release);
    RENDER_H.store(h, Ordering::Release);
}

static AF_LEVEL: AtomicU32 = AtomicU32::new(0);
static ORIG_SET_SAMPLER: AtomicUsize = AtomicUsize::new(0);
// Negative sharpens, at the cost of shimmer; positive softens.
static LOD_BIAS_SET: AtomicBool = AtomicBool::new(false);
static LOD_BIAS_BITS: AtomicU32 = AtomicU32::new(0);

/// Set before device creation.
pub fn set_anisotropic(level: u32) {
    AF_LEVEL.store(level, Ordering::Release);
}

/// `None` leaves the game's value. Set before device creation.
pub fn set_lod_bias(value: Option<f32>) {
    if let Some(v) = value {
        LOD_BIAS_BITS.store(v.to_bits(), Ordering::Release);
    }
    LOD_BIAS_SET.store(value.is_some(), Ordering::Release);
}

static MIPMAP_FIX: AtomicBool = AtomicBool::new(false);
static ORIG_CREATE_TEXTURE: AtomicUsize = AtomicUsize::new(0);

/// Set before device creation.
pub fn set_mipmap_fix(on: bool) {
    MIPMAP_FIX.store(on, Ordering::Release);
}

// Needs both the CreateTexture and SetTexture hooks.
static TEXTURE_MODS: AtomicBool = AtomicBool::new(false);
static ORIG_SET_TEXTURE: AtomicUsize = AtomicUsize::new(0);

/// Set before device creation.
pub fn set_texture_mods(on: bool) {
    TEXTURE_MODS.store(on, Ordering::Release);
}

/// Bypasses the hooked slot, so `texmod` neither recurses nor picks up the mipmap fixup.
///
/// # Safety
/// `device` must be a valid `IDirect3DDevice9`; `pp_texture` a writable out-ptr.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn orig_create_texture(
    device: *mut c_void,
    width: u32,
    height: u32,
    levels: u32,
    usage: u32,
    format: u32,
    pool: u32,
    pp_texture: *mut *mut c_void,
) -> i32 {
    unsafe {
        let orig: CreateTextureFn =
            core::mem::transmute(ORIG_CREATE_TEXTURE.load(Ordering::Acquire));
        orig(
            device,
            width,
            height,
            levels,
            usage,
            format,
            pool,
            pp_texture,
            core::ptr::null_mut(),
        )
    }
}

// A few seconds of visible confirmation that the proxy is live.
static STARTUP_OVERLAY: AtomicBool = AtomicBool::new(false);
static ORIG_END_SCENE: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_START: OnceLock<Instant> = OnceLock::new();
static OVERLAY_TEXT: OnceLock<String> = OnceLock::new();

/// Set before device creation.
pub fn set_startup_overlay(on: bool, text: &str) {
    STARTUP_OVERLAY.store(on, Ordering::Release);
    if on {
        let _ = OVERLAY_TEXT.set(text.to_string());
    }
}

/// One rect per lit pixel, all in a single `Clear`.
unsafe fn draw_text(this: *mut c_void, clear: ClearFn, text: &str, x0: i32, y0: i32, color: u32) {
    unsafe {
        let mut rects: Vec<Rect> = Vec::with_capacity(text.len() * 24);
        let mut cx = x0;
        for &ch in text.as_bytes() {
            for (gy, &row) in crate::font::FONT8X8[(ch & 0x7F) as usize]
                .iter()
                .enumerate()
            {
                for gx in 0..crate::font::GLYPH_W {
                    if row & (1 << gx) != 0 {
                        let px = cx + gx as i32 * OVERLAY_SCALE;
                        let py = y0 + gy as i32 * OVERLAY_SCALE;
                        rects.push(Rect {
                            left: px,
                            top: py,
                            right: px + OVERLAY_SCALE,
                            bottom: py + OVERLAY_SCALE,
                        });
                    }
                }
            }
            cx += crate::font::GLYPH_W as i32 * OVERLAY_SCALE;
        }
        if !rects.is_empty() {
            clear(
                this,
                rects.len() as u32,
                rects.as_ptr(),
                D3DCLEAR_TARGET,
                color,
                0.0,
                0,
            );
        }
    }
}

/// Drawn just before scene end, so it sits on top of everything else.
unsafe extern "system" fn end_scene_hook(this: *mut c_void) -> i32 {
    unsafe {
        let start = OVERLAY_START.get_or_init(Instant::now);
        if start.elapsed() < Duration::from_secs(OVERLAY_SECS)
            && let Some(text) = OVERLAY_TEXT.get()
        {
            let vtable = *(this as *const *const usize);
            let clear: ClearFn = core::mem::transmute(*vtable.add(DEV_CLEAR));
            let (x0, y0) = (16, 16);
            let w = text.len() as i32 * crate::font::GLYPH_W as i32 * OVERLAY_SCALE;
            let h = crate::font::GLYPH_H as i32 * OVERLAY_SCALE;
            let bg = Rect {
                left: x0 - 8,
                top: y0 - 8,
                right: x0 + w + 8,
                bottom: y0 + h + 8,
            };
            clear(this, 1, &bg, D3DCLEAR_TARGET, OVERLAY_BG, 0.0, 0);
            draw_text(this, clear, text, x0, y0, OVERLAY_COLOR);
        }
        let orig: EndSceneFn = core::mem::transmute(ORIG_END_SCENE.load(Ordering::Acquire));
        orig(this)
    }
}

static SCISSOR_W: AtomicU32 = AtomicU32::new(0x3F80_0000);
static SCISSOR_H: AtomicU32 = AtomicU32::new(0x3F80_0000);

static mut FIXED_VERTEX: [f32; 20] = [0.0; 20];
static FIXED_READY: AtomicBool = AtomicBool::new(false);

/// Hardcoded for 1280x720, so a match gets substituted with the resolution-correct version.
const EXPECTED_VERTEX: [f32; 20] = [
    -1.0 - 1.0 / 1280.0,
    1.0 + 1.0 / 720.0,
    0.0,
    0.0,
    0.0,
    1.0 - 1.0 / 1280.0,
    1.0 + 1.0 / 720.0,
    0.0,
    1.0,
    0.0,
    1.0 - 1.0 / 1280.0,
    -1.0 + 1.0 / 720.0,
    0.0,
    1.0,
    1.0,
    -1.0 - 1.0 / 1280.0,
    -1.0 + 1.0 / 720.0,
    0.0,
    0.0,
    1.0,
];

// Each sentinel means "leave the game's value"; see `apply_present_params`.
static PO_TRIPLE: AtomicU32 = AtomicU32::new(0);
static PO_INTERVAL: AtomicU32 = AtomicU32::new(0);
static PO_REFRESH: AtomicU32 = AtomicU32::new(u32::MAX);
static PO_MSAA: AtomicU32 = AtomicU32::new(0);
static PO_SWAP: AtomicU32 = AtomicU32::new(u32::MAX);

/// Configures the present-parameter tweaks applied in the CreateDevice hook.
pub fn set_present_options(triple: bool, interval: i32, refresh: i32, msaa: u32, swap: i32) {
    PO_TRIPLE.store(u32::from(triple), Ordering::Release);
    PO_INTERVAL.store(interval as u32, Ordering::Release);
    PO_REFRESH.store(
        if refresh < 0 {
            u32::MAX
        } else {
            refresh as u32
        },
        Ordering::Release,
    );
    PO_MSAA.store(msaa, Ordering::Release);
    PO_SWAP.store(
        if swap < 0 { u32::MAX } else { swap as u32 },
        Ordering::Release,
    );
}

unsafe fn apply_present_params(pp: *mut c_void) {
    unsafe {
        if pp.is_null() {
            return;
        }
        let wr = |off: usize, v: u32| {
            core::ptr::write_unaligned((pp as *mut u8).add(off) as *mut u32, v)
        };
        let rd = |off: usize| core::ptr::read_unaligned((pp as *const u8).add(off) as *const u32);
        if PO_TRIPLE.load(Ordering::Acquire) == 1 {
            wr(12, 3);
        }
        let refresh = PO_REFRESH.load(Ordering::Acquire);
        if refresh != u32::MAX && rd(48) != 0 {
            wr(48, refresh);
        }
        let msaa = PO_MSAA.load(Ordering::Acquire);
        if msaa > 0 {
            wr(24, 1);
            wr(16, msaa);
            wr(20, 0);
        }
        match PO_INTERVAL.load(Ordering::Acquire) as i32 {
            -1 => wr(52, 0x8000_0000),
            1 => wr(52, 1),
            _ => {}
        }
        let swap = PO_SWAP.load(Ordering::Acquire);
        if swap != u32::MAX {
            wr(24, swap);
        }
    }
}

/// Called from the patch thread, once the internal resolution has been read.
pub fn set_scissor_factors(w: f32, h: f32) {
    SCISSOR_W.store(w.to_bits(), Ordering::Release);
    SCISSOR_H.store(h.to_bits(), Ordering::Release);
}

/// Computes the resolution-correct UI vertex data from the internal resolution.
pub fn set_internal_resolution(width: u32, height: u32) {
    let base: [f32; 20] = [
        -1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, -1.0, -1.0,
        0.0, 0.0, 1.0,
    ];
    let (dw, dh) = (1.0 / width as f32, 1.0 / height as f32);
    let mut fixed = base;
    for row in 0..4 {
        fixed[row * 5] -= dw;
        fixed[row * 5 + 1] += dh;
    }
    unsafe {
        FIXED_VERTEX = fixed;
    }
    FIXED_READY.store(true, Ordering::Release);
}

/// Installs the `CreateDevice` hook on a freshly created `IDirect3D9`.
pub fn hook_d3d9(d3d9: *mut c_void) {
    if d3d9.is_null() || D3D9_HOOKED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::log::flog!("hook_d3d9: hooking CreateDevice on {:#x}", d3d9 as usize);
    unsafe {
        let orig = hook_slot(
            d3d9,
            IDIRECT3D9_CREATE_DEVICE,
            create_device_hook as *const c_void,
        );
        ORIG_CREATE_DEVICE.store(orig as usize, Ordering::Release);
    }
    crate::log::flog!("hook_d3d9: CreateDevice hooked");
}

unsafe extern "system" fn create_device_hook(
    this: *mut c_void,
    adapter: u32,
    device_type: u32,
    focus_window: *mut c_void,
    behavior_flags: u32,
    present_params: *mut c_void,
    returned_device: *mut *mut c_void,
) -> i32 {
    unsafe {
        crate::log::flog!("create_device_hook enter");
        if !present_params.is_null() {
            let rd = |o: usize| {
                core::ptr::read_unaligned((present_params as *const u8).add(o) as *const u32)
            };
            crate::log::flog!(
                "create_device_hook: game requested backbuffer={}x{} windowed={} refresh={}Hz",
                rd(0),
                rd(4),
                rd(32),
                rd(48)
            );
        }
        let (rw, rh) = (
            RENDER_W.load(Ordering::Acquire),
            RENDER_H.load(Ordering::Acquire),
        );
        if rw > 0 && rh > 0 && !present_params.is_null() {
            core::ptr::write_unaligned(present_params as *mut u32, rw);
            core::ptr::write_unaligned((present_params as *mut u8).add(4) as *mut u32, rh);
            let addr = RENDER_RES_ADDR.load(Ordering::Acquire);
            let mut old = 0u32;
            if addr != 0
                && VirtualProtect(addr as *mut c_void, 8, PAGE_EXECUTE_READWRITE, &mut old) != 0
            {
                core::ptr::write_unaligned(addr as *mut u32, rw);
                core::ptr::write_unaligned((addr + 4) as *mut u32, rh);
                let mut tmp = 0u32;
                VirtualProtect(addr as *mut c_void, 8, old, &mut tmp);
            }
            crate::log::flog!("create_device_hook: forcing render resolution {rw}x{rh}");
        }
        let orig: CreateDeviceFn = core::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Acquire));
        apply_present_params(present_params);
        crate::window::apply_present(present_params);
        let hr = orig(
            this,
            adapter,
            device_type,
            focus_window,
            behavior_flags,
            present_params,
            returned_device,
        );
        crate::log::flog!("create_device_hook: orig CreateDevice hr={hr:#x}");
        if !DEVICE_FIXES.load(Ordering::Acquire) {
            crate::log::flog!("create_device_hook: device_fixes disabled, skipping vtable hooks");
            return hr;
        }
        if hr >= 0 && !returned_device.is_null() {
            let device = *returned_device;
            if !device.is_null() && !DEVICE_HOOKED.swap(true, Ordering::AcqRel) {
                ORIG_SET_SCISSOR.store(
                    hook_slot(
                        device,
                        DEV_SET_SCISSOR_RECT,
                        set_scissor_hook as *const c_void,
                    ) as usize,
                    Ordering::Release,
                );
                ORIG_CREATE_VB.store(
                    hook_slot(
                        device,
                        DEV_CREATE_VERTEX_BUFFER,
                        create_vb_hook as *const c_void,
                    ) as usize,
                    Ordering::Release,
                );
                ORIG_DRAW_UP.store(
                    hook_slot(device, DEV_DRAW_PRIMITIVE_UP, draw_up_hook as *const c_void)
                        as usize,
                    Ordering::Release,
                );
                crate::log::flog!(
                    "create_device_hook: device vtable hooked (scissor={:#x} vb={:#x} drawup={:#x})",
                    ORIG_SET_SCISSOR.load(Ordering::Acquire),
                    ORIG_CREATE_VB.load(Ordering::Acquire),
                    ORIG_DRAW_UP.load(Ordering::Acquire)
                );
                if AF_LEVEL.load(Ordering::Acquire) > 1 || LOD_BIAS_SET.load(Ordering::Acquire) {
                    ORIG_SET_SAMPLER.store(
                        hook_slot(
                            device,
                            DEV_SET_SAMPLER_STATE,
                            set_sampler_hook as *const c_void,
                        ) as usize,
                        Ordering::Release,
                    );
                    crate::log::flog!(
                        "device: sampler hook (aniso={}x lod_bias_override={})",
                        AF_LEVEL.load(Ordering::Acquire),
                        LOD_BIAS_SET.load(Ordering::Acquire)
                    );
                }
                if MIPMAP_FIX.load(Ordering::Acquire) || TEXTURE_MODS.load(Ordering::Acquire) {
                    ORIG_CREATE_TEXTURE.store(
                        hook_slot(
                            device,
                            DEV_CREATE_TEXTURE,
                            create_texture_hook as *const c_void,
                        ) as usize,
                        Ordering::Release,
                    );
                    crate::log::flog!(
                        "device: CreateTexture hooked (mipmap_fix={} texture_mods={})",
                        MIPMAP_FIX.load(Ordering::Acquire),
                        TEXTURE_MODS.load(Ordering::Acquire)
                    );
                }
                if TEXTURE_MODS.load(Ordering::Acquire) {
                    ORIG_SET_TEXTURE.store(
                        hook_slot(device, DEV_SET_TEXTURE, set_texture_hook as *const c_void)
                            as usize,
                        Ordering::Release,
                    );
                    crate::log::flog!("device: LayeredFS texture swap enabled");
                }
                if STARTUP_OVERLAY.load(Ordering::Acquire) {
                    ORIG_END_SCENE.store(
                        hook_slot(device, DEV_END_SCENE, end_scene_hook as *const c_void) as usize,
                        Ordering::Release,
                    );
                    crate::log::flog!("device: EndScene hooked for the startup overlay");
                }
            }
        }
        hr
    }
}

unsafe extern "system" fn set_scissor_hook(this: *mut c_void, rect: *const Rect) -> i32 {
    unsafe {
        let first = !LOGGED_SCISSOR.swap(true, Ordering::AcqRel);
        let orig: SetScissorRectFn = core::mem::transmute(ORIG_SET_SCISSOR.load(Ordering::Acquire));
        if rect.is_null() {
            if first {
                crate::log::flog!("set_scissor_hook: first call (rect=null)");
            }
            return orig(this, rect);
        }
        let w = f32::from_bits(SCISSOR_W.load(Ordering::Acquire));
        let h = f32::from_bits(SCISSOR_H.load(Ordering::Acquire));
        let r = &*rect;
        if first {
            crate::log::flog!(
                "set_scissor_hook: first call (rect={},{},{},{} scale={w},{h})",
                r.left,
                r.top,
                r.right,
                r.bottom
            );
        }
        let scaled = Rect {
            left: (r.left as f32 * w) as i32,
            top: (r.top as f32 * h) as i32,
            right: (r.right as f32 * w) as i32,
            bottom: (r.bottom as f32 * h) as i32,
        };
        let hr = orig(this, &scaled);
        if first {
            crate::log::flog!("set_scissor_hook: first call returned {hr:#x}");
        }
        hr
    }
}

unsafe extern "system" fn create_vb_hook(
    this: *mut c_void,
    length: u32,
    mut usage: u32,
    fvf: u32,
    mut pool: u32,
    pp_vb: *mut *mut c_void,
    shared: *mut *mut c_void,
) -> i32 {
    unsafe {
        let first = !LOGGED_VB.swap(true, Ordering::AcqRel);
        let orig: CreateVertexBufferFn =
            core::mem::transmute(ORIG_CREATE_VB.load(Ordering::Acquire));
        if length == UI_VERTEX_BUFFER_LEN && pool == D3DPOOL_MANAGED {
            usage = D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY;
            pool = D3DPOOL_SYSTEMMEM;
        }
        if first {
            crate::log::flog!(
                "create_vb_hook: first call (len={length} usage={usage:#x} pool={pool})"
            );
        }
        let hr = orig(this, length, usage, fvf, pool, pp_vb, shared);
        if first {
            crate::log::flog!("create_vb_hook: first call returned {hr:#x}");
        }
        hr
    }
}

unsafe extern "system" fn draw_up_hook(
    this: *mut c_void,
    primitive_type: u32,
    primitive_count: u32,
    vertex_data: *const c_void,
    stride: u32,
) -> i32 {
    unsafe {
        let first = !LOGGED_DRAW.swap(true, Ordering::AcqRel);
        if first {
            crate::log::flog!(
                "draw_up_hook: first call (type={primitive_type} count={primitive_count} stride={stride})"
            );
        }
        let orig: DrawPrimitiveUpFn = core::mem::transmute(ORIG_DRAW_UP.load(Ordering::Acquire));
        if primitive_type == D3DPT_TRIANGLEFAN
            && primitive_count == 2
            && stride == 20
            && FIXED_READY.load(Ordering::Acquire)
            && matches_expected(vertex_data)
        {
            let fixed = core::ptr::addr_of!(FIXED_VERTEX) as *const c_void;
            return orig(this, primitive_type, primitive_count, fixed, stride);
        }
        let hr = orig(this, primitive_type, primitive_count, vertex_data, stride);
        if first {
            crate::log::flog!("draw_up_hook: first call returned {hr:#x}");
        }
        hr
    }
}

unsafe fn matches_expected(data: *const c_void) -> bool {
    unsafe {
        if data.is_null() {
            return false;
        }
        let incoming = core::slice::from_raw_parts(data as *const f32, 20);
        incoming
            .iter()
            .zip(EXPECTED_VERTEX.iter())
            .all(|(a, b)| (a - b).abs() <= 1e-6)
    }
}

/// Without a mip chain, anisotropic filtering cannot suppress shimmer. Render targets,
/// depth-stencils and system-memory textures are left alone.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_texture_hook(
    this: *mut c_void,
    width: u32,
    height: u32,
    mut levels: u32,
    mut usage: u32,
    format: u32,
    pool: u32,
    pp_texture: *mut *mut c_void,
    shared: *mut c_void,
) -> i32 {
    unsafe {
        let orig: CreateTextureFn =
            core::mem::transmute(ORIG_CREATE_TEXTURE.load(Ordering::Acquire));
        let fixup = MIPMAP_FIX.load(Ordering::Acquire)
            && levels == 1
            && usage & 0x3 == 0
            && pool != D3DPOOL_SYSTEMMEM
            && (0x100..=0x800).contains(&width)
            && (0x100..=0x800).contains(&height);
        if fixup {
            levels = 0;
            usage |= D3DUSAGE_AUTOGENMIPMAP;
        }
        let hr = orig(
            this, width, height, levels, usage, format, pool, pp_texture, shared,
        );
        if fixup && hr >= 0 && !pp_texture.is_null() {
            let tex = *pp_texture;
            if !tex.is_null() {
                let vtable = *(tex as *const *const usize);
                let set_filter: SetAutoGenFilterFn =
                    core::mem::transmute(*vtable.add(TEX_SET_AUTOGEN_FILTER));
                set_filter(tex, D3DTEXF_ANISOTROPIC);
                let gen_mips: GenerateMipsFn = core::mem::transmute(*vtable.add(TEX_GENERATE_MIPS));
                gen_mips(tex);
            }
        }
        // D3D reuses freed addresses, so without eviction a pool of reused objects all keep the
        // first one's cached override.
        if TEXTURE_MODS.load(Ordering::Acquire) && hr >= 0 && !pp_texture.is_null() {
            crate::texmod::invalidate(*pp_texture);
        }
        hr
    }
}

/// The quad is sized from the original's declared dims, so swapping only the GPU texture fills
/// it at higher resolution without resizing anything.
unsafe extern "system" fn set_texture_hook(
    this: *mut c_void,
    stage: u32,
    texture: *mut c_void,
) -> i32 {
    unsafe {
        let orig: SetTextureFn = core::mem::transmute(ORIG_SET_TEXTURE.load(Ordering::Acquire));
        orig(this, stage, crate::texmod::substitute(this, texture))
    }
}

/// Point-sampled minification is left alone, since that is what keeps the UI crisp.
unsafe extern "system" fn set_sampler_hook(
    this: *mut c_void,
    sampler: u32,
    ty: u32,
    mut value: u32,
) -> i32 {
    unsafe {
        let orig: SetSamplerStateFn =
            core::mem::transmute(ORIG_SET_SAMPLER.load(Ordering::Acquire));
        let level = AF_LEVEL.load(Ordering::Acquire);
        if level > 1 {
            match ty {
                D3DSAMP_MINFILTER if value == D3DTEXF_LINEAR => {
                    orig(this, sampler, D3DSAMP_MAXANISOTROPY, level);
                    value = D3DTEXF_ANISOTROPIC;
                }
                D3DSAMP_MIPFILTER => value = D3DTEXF_LINEAR,
                D3DSAMP_MAXANISOTROPY => value = level,
                _ => {}
            }
        }
        if LOD_BIAS_SET.load(Ordering::Acquire) && ty == D3DSAMP_MIPMAPLODBIAS {
            value = LOD_BIAS_BITS.load(Ordering::Acquire);
        }
        orig(this, sampler, ty, value)
    }
}

/// Publishes the original before splicing, so a call arriving on another thread through a shared
/// vtable never sees a null.
pub(crate) unsafe fn hook_slot_store(
    obj: *mut c_void,
    index: usize,
    hook: *const c_void,
    orig: &AtomicUsize,
) {
    unsafe {
        let vtable = *(obj as *const *mut *const c_void);
        orig.store(*vtable.add(index) as usize, Ordering::Release);
        hook_slot(obj, index, hook);
    }
}

pub(crate) unsafe fn hook_slot(
    obj: *mut c_void,
    index: usize,
    hook: *const c_void,
) -> *const c_void {
    unsafe {
        let vtable = *(obj as *const *mut *const c_void);
        let slot = vtable.add(index);
        let original = *slot;
        let mut old = 0u32;
        if VirtualProtect(
            slot as *mut c_void,
            core::mem::size_of::<*const c_void>(),
            PAGE_EXECUTE_READWRITE,
            &mut old,
        ) != 0
        {
            *slot = hook;
            let mut restored = 0u32;
            VirtualProtect(
                slot as *mut c_void,
                core::mem::size_of::<*const c_void>(),
                old,
                &mut restored,
            );
        }
        original
    }
}
