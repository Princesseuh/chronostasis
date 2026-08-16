use std::path::PathBuf;

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_GUARD,
    PAGE_NOACCESS, VirtualProtect, VirtualQuery,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
};

use crate::dll_dir;
use crate::window::GAME_WINDOW_CLASS;

const IDYES: u32 = 6;

/// The shared morph-weight ramp's step has no dt term, so every morph through it, blinks and
/// lip-sync alike, is calibrated for 30fps and runs far too fast above it. Redirecting that one
/// instruction through a cave rescales the step, leaving 30fps exactly as it was.
struct FacialOffsets {
    morph_advance: usize,
    /// Where to resume after the original instruction.
    morph_ret: usize,
    /// In units of 1/300000 s.
    frame_delta: usize,
    const_30f: usize,
    const_300000f: usize,
}

/// NOPs the in-game frame-rate setter, then raises the frame-pacer floats.
struct FramerateOffsets {
    set_instr: usize,
    pacer_ptr: usize,
}

/// NOPs where the game zeroes its vibration floats, so they can be read back and fed to XInput.
struct VibrationOffsets {
    low_zero: usize,
    high_zero: usize,
    input_ptr: usize,
}

/// Rewrites the language routine to return a fixed code. The instruction shape differs per build,
/// so each game needs its own variant.
enum LangPatch {
    /// The flag byte is 1 for EN through JP and 0 for CN/KR.
    Xiii {
        flag: usize,
        b: usize,
        c705: usize,
        code: usize,
    },
    /// No flag byte, and a shorter tail than FFXIII's.
    Xiii2 { b: usize, c705: usize, code: usize },
    /// A single site rather than a split flag and code.
    Lr { site: usize },
}

/// A modal box deadlocks under DXVK emulated fullscreen, so the call is replaced with a fixed
/// "yes" and close resolves immediately.
struct MsgBoxOffsets {
    stack_push: usize,
    call: usize,
    /// `(offset from `stack_push`, len)`; the push encoding differs per build.
    push_nops: &'static [(usize, usize)],
}

/// An empty or `None` slot is a site that is absent or unknown for that game, and its fix is
/// skipped.
struct GameOffsets {
    framerate: Option<FramerateOffsets>,
    continuous_scan_instr: Option<usize>,
    /// Height follows at +4.
    internal_res_w: Option<usize>,
    vibration: Option<VibrationOffsets>,
    /// Forces the loose-file path. The target byte is only written once `.text` is decrypted.
    unpacked: &'static [(usize, u8)],
    /// The game's own scissor scaling, NOPed so the `SetScissorRect` hook can do it instead.
    scissor_nops: &'static [(usize, usize)],
    facial: Option<FacialOffsets>,
    lang: Option<LangPatch>,
    debug: &'static [(usize, &'static [u8])],
    msgbox: Option<MsgBoxOffsets>,
}

/// Validated in-game under Proton.
static FF13: GameOffsets = GameOffsets {
    framerate: Some(FramerateOffsets {
        set_instr: 0xA8D65F,
        pacer_ptr: 0x243E34C,
    }),
    continuous_scan_instr: Some(0x420868),
    internal_res_w: Some(0x22E5168),
    vibration: Some(VibrationOffsets {
        low_zero: 0x4210DF,
        high_zero: 0x4210F3,
        input_ptr: 0x0241_1220,
    }),
    unpacked: &[(0x3135, 0xEB), (0x92FA, 0xEB)],
    scissor_nops: &[
        (0x616596, 3),
        (0x6165BB, 3),
        (0x61654C, 3),
        (0x616571, 3),
        (0x572B26, 5),
        (0x668DE9, 4),
        (0x668E1E, 7),
        (0x668E56, 7),
        (0x668E91, 7),
    ],
    facial: Some(FacialOffsets {
        morph_advance: 0x7A75AD,
        morph_ret: 0x7A75B2,
        frame_delta: 0x246DE00,
        const_30f: 0xD4788C,
        const_300000f: 0xC9B134,
    }),
    lang: Some(LangPatch::Xiii {
        flag: 0x75FE,
        b: 0x436A8C,
        c705: 0x436AAB,
        code: 0x436AB1,
    }),
    debug: &[
        (0x95C7, &[0]),
        (0x97C1, &[0]),
        (0x97F0, &[0]),
        (0x98BE, &[0]),
    ],
    msgbox: Some(MsgBoxOffsets {
        stack_push: 0xA8A982,
        call: 0xA8A98F,
        push_nops: &[(0, 1), (4, 1), (8, 1), (12, 1)],
    }),
};

/// Untested in-game, and missing the scissor, resolution and facial-fix sites.
static FF13_2: GameOffsets = GameOffsets {
    framerate: Some(FramerateOffsets {
        set_instr: 0x802616,
        pacer_ptr: 0x4D67208,
    }),
    continuous_scan_instr: Some(0x2A6E7F),
    internal_res_w: Some(0x1FA864C),
    vibration: Some(VibrationOffsets {
        low_zero: 0x2A7221,
        high_zero: 0x2A7226,
        input_ptr: 0x212A164,
    }),
    unpacked: &[(0x9884, 0x75), (0xE829, 0xEB)],
    scissor_nops: &[],
    facial: None,
    lang: Some(LangPatch::Xiii2 {
        b: 0x2B2718,
        c705: 0x2B2733,
        code: 0x2B2739,
    }),
    debug: &[
        (0xE978, &[0]),
        (0xE9B8, &[0]),
        (0xE9EC, &[0xFF, 0xFF, 0xFF, 0xFF]),
    ],
    msgbox: Some(MsgBoxOffsets {
        stack_push: 0x8047B4,
        call: 0x8047C0,
        push_nops: &[(0, 5)],
    }),
};

/// Only unpacked-mode and language are known; LR has no debug menu at all.
static FF13_LR: GameOffsets = GameOffsets {
    framerate: None,
    continuous_scan_instr: None,
    internal_res_w: None,
    vibration: None,
    unpacked: &[(0x34799, 0xEB)],
    scissor_nops: &[],
    facial: None,
    lang: Some(LangPatch::Lr { site: 0x353DEE }),
    debug: &[],
    msgbox: None,
};

/// `None` on an unsupported build, which skips the memory patches but keeps the framework hooks.
fn offsets(game: DetectedGame) -> Option<&'static GameOffsets> {
    match game {
        DetectedGame::Xiii => Some(&FF13),
        DetectedGame::Xiii2 => Some(&FF13_2),
        DetectedGame::Lr => Some(&FF13_LR),
        DetectedGame::Other => None,
    }
}

/// The code byte, plus whether it is Chinese or Korean.
fn language_code(name: &str) -> Option<(u8, bool)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "en" => Some((0x01, false)),
        "fr" => Some((0x05, false)),
        "de" => Some((0x04, false)),
        "it" => Some((0x03, false)),
        "es" => Some((0x06, false)),
        "jp" => Some((0x00, false)),
        "cn" => Some((0x0A, true)),
        "kr" => Some((0x08, true)),
        _ => None,
    }
}

const MAX_FRAME_RATE_LIMIT: f32 = 250000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectedGame {
    Xiii,
    Xiii2,
    Lr,
    Other,
}

struct Config {
    framerate_uncap: bool,
    /// 0 means no cap.
    frame_rate_limit: u32,
    facial_anim_fix: bool,
    controller_scan_fix: bool,
    controller_hotplug: bool,
    /// Recombines the Xbox One trigger axes DirectInput splits.
    controller_trigger_fix: bool,
    device_fixes: bool,
    unpacked_mode: bool,
    debug_mode: bool,
    text_language: Option<(u8, bool)>,
    vibration: bool,
    vibration_strength: f32,
    confirm_exit: bool,
    triple_buffering: bool,
    /// 1 is vsync on, -1 off, 0 leaves it alone.
    present_interval: i32,
    /// Negative leaves the game's refresh rate.
    fullscreen_refresh: i32,
    multisample: u32,
    /// -1 leaves the game's swap effect.
    swap_effect: i32,
    /// 0 keeps the game's choice.
    render_w: u32,
    render_h: u32,
    /// 0 and 1 are off.
    anisotropic: u32,
    /// Auto-generates mip chains, so anisotropic filtering stops shimmering.
    experimental_mipmap_fix: bool,
    lod_bias: Option<f32>,
    startup_overlay: bool,
    texture_mods: bool,
    /// Same-dimension swaps only; anything larger resizes the HUD.
    unsafe_gui_overlay: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            framerate_uncap: true,
            frame_rate_limit: 0,
            facial_anim_fix: true,
            controller_scan_fix: true,
            controller_hotplug: true,
            controller_trigger_fix: false,
            device_fixes: true,
            unpacked_mode: false,
            debug_mode: false,
            text_language: None,
            vibration: true,
            vibration_strength: 2.0,
            confirm_exit: true,
            triple_buffering: false,
            present_interval: 0,
            fullscreen_refresh: -1,
            multisample: 0,
            swap_effect: -1,
            render_w: 0,
            render_h: 0,
            anisotropic: 0,
            experimental_mipmap_fix: true,
            lod_bias: None,
            startup_overlay: true,
            texture_mods: true,
            unsafe_gui_overlay: false,
        }
    }
}

/// Keys arrive lowercased and both sides trimmed.
pub(crate) fn for_each_ini_entry(dir: Option<&PathBuf>, mut f: impl FnMut(&str, &str)) {
    let Some(dir) = dir else { return };
    let Ok(text) = std::fs::read_to_string(dir.join("chronostasis.ini")) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        f(&k.trim().to_ascii_lowercase(), v.trim());
    }
}

/// An unrecognised value keeps `current` and is logged.
pub(crate) fn parse_bool(key: &str, value: &str, current: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        other => {
            flog!("config: {key} = {other:?} is not a boolean; keeping {current}");
            current
        }
    }
}

impl Config {
    fn load(dir: Option<&PathBuf>) -> Config {
        let mut cfg = Config::default();
        for_each_ini_entry(dir, |key, v| {
            let on = |current: bool| parse_bool(key, v, current);
            match key {
                "framerate_uncap" => cfg.framerate_uncap = on(cfg.framerate_uncap),
                "frame_rate_limit" => cfg.frame_rate_limit = v.parse().unwrap_or(0),
                "facial_anim_fix" => cfg.facial_anim_fix = on(cfg.facial_anim_fix),
                "controller_scan_fix" => cfg.controller_scan_fix = on(cfg.controller_scan_fix),
                "controller_hotplug" => cfg.controller_hotplug = on(cfg.controller_hotplug),
                "controller_trigger_fix" => {
                    cfg.controller_trigger_fix = on(cfg.controller_trigger_fix)
                }
                "device_fixes" => cfg.device_fixes = on(cfg.device_fixes),
                "unpacked_mode" => cfg.unpacked_mode = on(cfg.unpacked_mode),
                "debug_mode" => cfg.debug_mode = on(cfg.debug_mode),
                "text_language" => cfg.text_language = language_code(v),
                "vibration" => cfg.vibration = on(cfg.vibration),
                "vibration_strength" => cfg.vibration_strength = v.parse().unwrap_or(2.0),
                "confirm_exit" => cfg.confirm_exit = on(cfg.confirm_exit),
                "triple_buffering" => cfg.triple_buffering = on(cfg.triple_buffering),
                "present_interval" => cfg.present_interval = v.parse().unwrap_or(0),
                "fullscreen_refresh" => cfg.fullscreen_refresh = v.parse().unwrap_or(-1),
                "multisample" => cfg.multisample = v.parse().unwrap_or(0),
                "swap_effect" => cfg.swap_effect = v.parse().unwrap_or(-1),
                "render_resolution" => {
                    if let Some((w, h)) = v.split_once(['x', 'X']) {
                        cfg.render_w = w.trim().parse().unwrap_or(0);
                        cfg.render_h = h.trim().parse().unwrap_or(0);
                    }
                }
                "anisotropic" => cfg.anisotropic = v.parse().unwrap_or(0),
                "experimental_mipmap_fix" => {
                    cfg.experimental_mipmap_fix = on(cfg.experimental_mipmap_fix)
                }
                "lod_bias" => cfg.lod_bias = v.parse::<f32>().ok(),
                "startup_overlay" => cfg.startup_overlay = on(cfg.startup_overlay),
                "texture_mods" => cfg.texture_mods = on(cfg.texture_mods),
                "unsafe_gui_overlay" => cfg.unsafe_gui_overlay = on(cfg.unsafe_gui_overlay),
                _ => {}
            }
        });
        cfg
    }
}

/// Uppercase, which reads cleaner in the 8x8 font.
fn overlay_text(cfg: &Config) -> String {
    let mut s = String::from("CHRONOSTASIS");
    if cfg.unpacked_mode {
        s.push_str(" UNPACKED");
    }
    if cfg.framerate_uncap {
        if cfg.frame_rate_limit > 0 {
            s.push_str(&format!(" FPS{}", cfg.frame_rate_limit));
        } else {
            s.push_str(" FPS+");
        }
    }
    if cfg.facial_anim_fix {
        s.push_str(" FACE");
    }
    if cfg.anisotropic > 1 {
        s.push_str(&format!(" AF{}X", cfg.anisotropic));
    }
    if cfg.experimental_mipmap_fix {
        s.push_str(" MIP");
    }
    if let Some(b) = cfg.lod_bias {
        s.push_str(&format!(" LOD{b}"));
    }
    if cfg.render_w > 0 {
        s.push_str(&format!(" {}X{}", cfg.render_w, cfg.render_h));
    }
    if cfg.vibration {
        s.push_str(" VIB");
    }
    s
}

pub fn run() {
    crate::log::flush_deferred();
    flog!("run: thread started");
    let dir = dll_dir();
    let cfg = Config::load(dir.as_ref());
    flog!(
        "run: config loaded (framerate={} scan_fix={} unpacked={} vibration={} device_fixes={} lang={:?})",
        cfg.framerate_uncap,
        cfg.controller_scan_fix,
        cfg.unpacked_mode,
        cfg.vibration,
        cfg.device_fixes,
        cfg.text_language.map(|(c, _)| c)
    );
    // These two are init-time: they must land before the file-system code reads them, which is
    // long before the window exists, so this thread races init instead of waiting.
    UNPACKED_FLAG.store(if cfg.unpacked_mode { 1 } else { 2 }, Ordering::Release);
    DEBUG_FLAG.store(if cfg.debug_mode { 1 } else { 2 }, Ordering::Release);
    // Spawned even with nothing to patch, since it also retires the CRT-init detours.
    std::thread::spawn(early_patch_poll);
    crate::window::configure_and_install(dir.as_ref());
    flog!("run: window::configure_and_install done");
    // Must precede the input system, and is API-only, so it is safe this early.
    crate::dinput::install(cfg.controller_trigger_fix);
    flog!(
        "run: dinput::install done (trigger_fix={})",
        cfg.controller_trigger_fix
    );
    // Safe now: these only store config, and touch no still-encrypted game code.
    crate::device::set_device_fixes(cfg.device_fixes);
    crate::device::set_anisotropic(cfg.anisotropic);
    crate::device::set_mipmap_fix(cfg.experimental_mipmap_fix);
    crate::device::set_lod_bias(cfg.lod_bias);
    crate::device::set_startup_overlay(cfg.startup_overlay, &overlay_text(&cfg));
    crate::device::set_present_options(
        cfg.triple_buffering,
        cfg.present_interval,
        cfg.fullscreen_refresh,
        cfg.multisample,
        cfg.swap_effect,
    );
    // Must be in place before any texture loads, and is pointless without unpacked mode, since
    // loose files are only read then.
    if cfg.texture_mods && cfg.unpacked_mode {
        match mods_root() {
            Some(root) => {
                crate::device::set_texture_mods(true);
                crate::texmod::configure(root, cfg.unsafe_gui_overlay);
                crate::laa::enable_texture_mods();
                flog!("run: texture mods (LayeredFS) enabled");
            }
            None => flog!("run: texture mods on but mods root not found, skipping"),
        }
    } else if cfg.texture_mods {
        flog!("run: texture mods need unpacked_mode; skipping");
    }
    let game = detect_game();
    let offs = offsets(game);
    flog!(
        "run: detect_game = {:?} (offsets known: {})",
        game,
        offs.is_some()
    );
    if cfg.render_w > 0 && cfg.render_h > 0 {
        match offs.and_then(|o| o.internal_res_w) {
            Some(res_w) => {
                crate::device::set_render_override(
                    module_base() + res_w,
                    cfg.render_w,
                    cfg.render_h,
                );
                flog!(
                    "run: render resolution override {}x{}",
                    cfg.render_w,
                    cfg.render_h
                );
            }
            None => flog!(
                "run: render override requested but internal-res offset unknown for {game:?}; skipping"
            ),
        }
    }
    let Some(o) = offs else {
        flog!("run: no offsets for {game:?}, skipping memory patches");
        return;
    };
    // Patching `.text` before SteamStub decrypts it corrupts the image and crashes.
    if !wait_for_game_window() {
        flog!(
            "run: game window never appeared; skipping memory patches (.text may still be encrypted)"
        );
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(2000));
    flog!("run: apply_fixes begin");
    unsafe { apply_fixes(o, &cfg, dir.as_ref()) };
    flog!("run: apply_fixes done");
}

/// Polls for the main window, so patches land after the image is decrypted and initialized.
fn wait_for_game_window() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
    let class: Vec<u16> = GAME_WINDOW_CLASS.encode_utf16().chain(Some(0)).collect();
    for _ in 0..1200 {
        let h = unsafe { FindWindowW(class.as_ptr(), core::ptr::null()) };
        if h as usize != 0 {
            flog!("run: found game window {:#x}", h as usize);
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// Cached, because this is polled from the `QueryPerformanceCounter` detour.
fn detect_game() -> DetectedGame {
    static CACHE: AtomicU8 = AtomicU8::new(0);
    match CACHE.load(Ordering::Acquire) {
        1 => return DetectedGame::Xiii,
        2 => return DetectedGame::Xiii2,
        3 => return DetectedGame::Lr,
        4 => return DetectedGame::Other,
        _ => {}
    }
    let Some(exe) = std::env::current_exe().ok().and_then(|p| {
        p.file_name()
            .map(|f| f.to_string_lossy().to_ascii_lowercase())
    }) else {
        return DetectedGame::Other;
    };
    let game = match exe.as_str() {
        "ffxiiiimg.exe" => DetectedGame::Xiii,
        "ffxiii2img.exe" => DetectedGame::Xiii2,
        "lrff13.exe" => DetectedGame::Lr,
        _ => DetectedGame::Other,
    };
    CACHE.store(
        match game {
            DetectedGame::Xiii => 1,
            DetectedGame::Xiii2 => 2,
            DetectedGame::Lr => 3,
            DetectedGame::Other => 4,
        },
        Ordering::Release,
    );
    game
}

/// Applied only once the window exists and the SteamStub image is decrypted; earlier would
/// corrupt it.
unsafe fn apply_fixes(o: &GameOffsets, cfg: &Config, dir: Option<&PathBuf>) {
    unsafe {
        let base = module_base();
        flog!("apply_fixes: base = {:#x}", base);
        if base == 0 {
            return;
        }
        if cfg.framerate_uncap {
            match &o.framerate {
                Some(fr) => {
                    nop_site_hot("framerate setter", base + fr.set_instr, 5, Expect::Code);
                }
                None => flog!("  framerate uncap unsupported for this game; skipping"),
            }
        }
        if cfg.controller_scan_fix {
            match o.continuous_scan_instr {
                Some(off) => {
                    let addr = base + off;
                    let orig = pre_bytes(addr, 1).map(|b| b[0]);
                    // Turning the scan into an unconditional skip removes a per-second stutter.
                    if patch_site(
                        "controller_scan",
                        addr,
                        &[0xEB],
                        Expect::First(&[0x75, 0xEB]),
                    ) && let (true, Some(orig)) = (cfg.controller_hotplug, orig)
                    {
                        if orig == 0xEB {
                            flog!(
                                "  controller hotplug: site already read 0xEB, so scan can't be re-enabled; monitor not started"
                            );
                        } else {
                            flog!("  spawn controller hotplug monitor");
                            std::thread::spawn(move || controller_hotplug_monitor(addr, orig));
                        }
                    }
                }
                None => flog!("  controller_scan_fix unsupported for this game; skipping"),
            }
        }
        if cfg.unpacked_mode {
            if o.unpacked.is_empty() {
                flog!("  unpacked_mode unsupported for this game; skipping");
            } else {
                for &(off, byte) in o.unpacked {
                    let addr = base + off;
                    if pre_bytes(addr, 1).is_some_and(|b| b[0] == byte) {
                        flog!("  unpacked_mode @ {addr:#x}: already applied");
                        continue;
                    }
                    patch_site("unpacked_mode", addr, &[byte], Expect::First(SHORT_JCC));
                }
            }
        }
        if let Some((code, cn_kr)) = cfg.text_language {
            match &o.lang {
                Some(LangPatch::Xiii {
                    flag,
                    b,
                    c705,
                    code: code_at,
                }) => {
                    flog!("  patch text_language (XIII) code={code:#x}");
                    patch_hot_group(
                        "text_language (XIII)",
                        &[
                            HotSite {
                                what: "lang flag",
                                addr: base + flag,
                                bytes: &[u8::from(!cn_kr)],
                                expect: Expect::Data,
                            },
                            HotSite {
                                what: "lang b",
                                addr: base + b,
                                bytes: &[0x28],
                                expect: Expect::Code,
                            },
                            HotSite {
                                what: "lang c705",
                                addr: base + c705,
                                bytes: &[0xC7, 0x05],
                                expect: Expect::Code,
                            },
                            HotSite {
                                what: "lang code",
                                addr: base + code_at,
                                bytes: &[code, 0, 0, 0, 0x8B, 0xE5, 0x5D, 0xC3],
                                expect: Expect::Code,
                            },
                        ],
                    );
                }
                Some(LangPatch::Xiii2 {
                    b,
                    c705,
                    code: code_at,
                }) => {
                    flog!("  patch text_language (XIII-2) code={code:#x}");
                    patch_hot_group(
                        "text_language (XIII-2)",
                        &[
                            HotSite {
                                what: "lang b",
                                addr: base + b,
                                bytes: &[0x24],
                                expect: Expect::Code,
                            },
                            HotSite {
                                what: "lang c705",
                                addr: base + c705,
                                bytes: &[0xC7, 0x05],
                                expect: Expect::Code,
                            },
                            HotSite {
                                what: "lang code",
                                addr: base + code_at,
                                bytes: &[code, 0, 0, 0, 0xC3],
                                expect: Expect::Code,
                            },
                        ],
                    );
                }
                Some(LangPatch::Lr { site }) => {
                    flog!("  patch text_language (LR) code={code:#x}");
                    patch_site_hot(
                        "lang site",
                        base + site,
                        &[0x90, 0x90, 0xB9, code, 0, 0, 0],
                        Expect::Code,
                    );
                }
                None => flog!("  text_language unsupported for this game; skipping"),
            }
        }
        if cfg.debug_mode {
            if o.debug.is_empty() {
                flog!("  debug_mode unsupported for this game; skipping");
            } else {
                for &(off, bytes) in o.debug {
                    // Config flags rather than code, so any prior value is plausible.
                    patch_site("debug_mode", base + off, bytes, Expect::Data);
                }
            }
        }
        if cfg.vibration {
            match &o.vibration {
                Some(v) => {
                    let low = nop_site_hot("vibration low", base + v.low_zero, 5, Expect::Code);
                    let high = nop_site_hot("vibration high", base + v.high_zero, 5, Expect::Code);
                    if low && high {
                        let ptr_loc = base + v.input_ptr;
                        let strength = cfg.vibration_strength;
                        std::thread::spawn(move || vibration_loop(ptr_loc, strength));
                    } else {
                        flog!("  vibration: zeroing NOPs did not land; rumble loop not started");
                    }
                }
                None => flog!("  vibration unsupported for this game; skipping"),
            }
        }

        if !cfg.confirm_exit {
            match &o.msgbox {
                Some(m) => remove_exit_messagebox(base, m),
                None => flog!("  exit-confirm messagebox unsupported for this game; skipping"),
            }
        }

        if cfg.facial_anim_fix {
            match &o.facial {
                Some(f) => {
                    flog!("  install facial_anim_fix");
                    install_facial_anim_fix(base, f);
                }
                None => flog!("  facial_anim_fix unsupported for this game; skipping"),
            }
        }

        flog!("  apply_nccp_patches");
        apply_nccp_patches(base, dir);

        // The pointer is only valid once running. Raising [0] stops the pacer throttling, and [1]
        // must move too or the game still clamps at 60.
        if cfg.framerate_uncap
            && let Some(fr) = &o.framerate
        {
            let pacer_ptr = *((base + fr.pacer_ptr) as *const usize);
            flog!("  frame pacer ptr = {pacer_ptr:#x}");
            if pacer_ptr != 0 {
                let limit = if cfg.frame_rate_limit == 0 {
                    MAX_FRAME_RATE_LIMIT
                } else {
                    (cfg.frame_rate_limit as f32).min(MAX_FRAME_RATE_LIMIT)
                };
                flog!("  frame rate limit = {limit}");
                patch_site(
                    "pacer target",
                    pacer_ptr,
                    &MAX_FRAME_RATE_LIMIT.to_le_bytes(),
                    Expect::Data,
                );
                patch_site(
                    "pacer limit",
                    pacer_ptr + 4,
                    &limit.to_le_bytes(),
                    Expect::Data,
                );
            }
        }

        if !cfg.device_fixes {
            flog!("  device_fixes disabled, skipping scissor/resolution");
            return;
        }
        // With the game's own scissor scaling neutered, the device hooks do it instead.
        if o.scissor_nops.is_empty() {
            flog!("  scissor/resolution fix not ported for this game; skipping");
            return;
        }
        let Some(res_off) = o.internal_res_w else {
            flog!("  scissor NOPs set but internal-res offset missing; skipping");
            return;
        };
        let res_w = *((base + res_off) as *const u32);
        let res_h = *((base + res_off + 4) as *const u32);
        flog!("  internal res = {res_w}x{res_h}");
        if res_w > 0 && res_h > 0 {
            for &(off, len) in o.scissor_nops {
                nop_site_hot("scissor scaling", base + off, len, Expect::Code);
            }
            crate::device::set_scissor_factors(res_w as f32 / 1280.0, res_h as f32 / 720.0);
            crate::device::set_internal_resolution(res_w, res_h);
        }
    }
}

/// Every site is verified before any is written, since a half-applied patch leaves a broken call.
unsafe fn remove_exit_messagebox(base: usize, m: &MsgBoxOffsets) {
    unsafe {
        static NOPS: [u8; 8] = [0x90; 8];
        let [a, b, c, d] = IDYES.to_le_bytes();
        let call_bytes = [0xB8, a, b, c, d, 0x90];
        let mut sites: Vec<HotSite> = Vec::with_capacity(m.push_nops.len() + 1);
        for &(rel, len) in m.push_nops {
            let Some(filler) = NOPS.get(..len) else {
                flog!("  exit-confirm push: {len}-byte NOP too long; leaving the dialog in place");
                return;
            };
            sites.push(HotSite {
                what: "exit-confirm push",
                addr: base + m.stack_push + rel,
                bytes: filler,
                expect: Expect::First(if len == 1 { PUSH_REG } else { PUSH_IMM32 }),
            });
        }
        sites.push(HotSite {
            what: "exit-confirm call",
            addr: base + m.call,
            bytes: &call_bytes,
            expect: Expect::First(CALL_OPCODES),
        });
        if patch_hot_group("exit-confirm messagebox", &sites) {
            flog!("  removed exit-confirm messagebox");
        }
    }
}

/// Bit `i` is set when slot `i` is connected.
fn xinput_connected_mask() -> u8 {
    use windows_sys::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};
    let mut mask = 0u8;
    for i in 0..4u32 {
        let mut state: XINPUT_STATE = unsafe { core::mem::zeroed() };
        if unsafe { XInputGetState(i, &mut state) } == 0 {
            mask |= 1 << i;
        }
    }
    mask
}

/// The continuous scan stays disabled to avoid its stutter, so this briefly re-enables it when a
/// new controller appears.
fn controller_hotplug_monitor(scan_addr: usize, scan_enabled_byte: u8) {
    use std::time::Duration;
    let mut connected = xinput_connected_mask();
    loop {
        std::thread::sleep(Duration::from_millis(2000));
        let now = xinput_connected_mask();
        let newly = now & !connected;
        connected = now;
        if newly != 0 {
            flog!("hotplug: new controller (mask {now:#x}); re-enabling scan briefly");
            unsafe {
                patch("hotplug scan on", scan_addr, &[scan_enabled_byte]);
                std::thread::sleep(Duration::from_millis(2500));
                patch("hotplug scan off", scan_addr, &[0xEB]);
            }
        }
    }
}

/// Re-reads the input pointer and re-scans for a pad every pass, so hotplug keeps working in
/// either direction.
fn vibration_loop(ptr_loc: usize, strength: f32) {
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::XboxController::{XINPUT_VIBRATION, XInputSetState};
    const VIB_LOW_OFFSET: usize = 0x9C;
    let scale = |x: f32| ((strength * x).min(1.0) * 65535.0) as u16;
    let mut controller: Option<u32> = None;
    let mut was_vibrating = false;
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let Some(pad) = controller.or_else(first_connected_pad) else {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        };
        let set = |l: u16, r: u16| {
            let v = XINPUT_VIBRATION {
                wLeftMotorSpeed: l,
                wRightMotorSpeed: r,
            };
            unsafe { XInputSetState(pad, &v) == 0 }
        };
        if controller.is_none() {
            controller = Some(pad);
            was_vibrating = false;
            set(0, 0);
        }
        let input = unsafe { core::ptr::read_volatile(ptr_loc as *const usize) };
        if input == 0 {
            continue;
        }
        let vib_low = (input + VIB_LOW_OFFSET) as *const f32;
        let lo = unsafe { core::ptr::read_volatile(vib_low) };
        let hi = unsafe { core::ptr::read_volatile(vib_low.add(1)) };
        let alive = if lo > 0.01 || hi > 0.01 {
            was_vibrating = true;
            set(scale(lo), scale(hi))
        } else if was_vibrating {
            was_vibrating = false;
            set(0, 0)
        } else {
            true
        };
        if !alive {
            flog!("vibration: pad {pad} gone; waiting for a controller");
            controller = None;
        }
    }
}

fn first_connected_pad() -> Option<u32> {
    use windows_sys::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};
    (0..4u32).find(|&i| {
        let mut state: XINPUT_STATE = unsafe { core::mem::zeroed() };
        unsafe { XInputGetState(i, &mut state) == 0 }
    })
}

/// A `.nccp` is a Valve VDF whose digit-keyed values are `"<hexAddr>|<hexBytes>"`.
unsafe fn apply_nccp_patches(base: usize, dir: Option<&PathBuf>) {
    unsafe {
        let Some(dir) = dir else { return };
        let patch_dir = dir.join("ff13-patches");
        let Ok(entries) = std::fs::read_dir(&patch_dir) else {
            return;
        };
        let image_size = module_image_size(base);
        if image_size == 0 {
            flog!("  nccp: module image size unreadable; skipping all .nccp patches");
            return;
        }
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("nccp") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let parsed = parse_nccp(&text);
            let mut in_range = true;
            for (addr, bytes) in &parsed {
                let end = addr.checked_add(bytes.len());
                if !end.is_some_and(|e| e <= image_size) {
                    flog!(
                        "  nccp {name}: {addr:#x}+{} is outside the module image ({image_size:#x}); whole file refused",
                        bytes.len()
                    );
                    in_range = false;
                }
            }
            if !in_range || parsed.is_empty() {
                continue;
            }
            let whats: Vec<String> = parsed
                .iter()
                .map(|(addr, _)| format!("nccp {name} {addr:#x}"))
                .collect();
            let sites: Vec<HotSite> = parsed
                .iter()
                .zip(&whats)
                .map(|((addr, bytes), what)| HotSite {
                    what,
                    addr: base + addr,
                    bytes: bytes.as_slice(),
                    expect: Expect::Data,
                })
                .collect();
            let group = format!("nccp {name}");
            patch_hot_group(&group, &sites);
        }
    }
}

/// 0 when the PE headers do not parse.
unsafe fn module_image_size(base: usize) -> usize {
    unsafe {
        if base == 0 || !readable(base, 0x40) || *(base as *const u16) != 0x5A4D {
            return 0;
        }
        let e_lfanew = *((base + 0x3C) as *const u32) as usize;
        let Some(nt) = base.checked_add(e_lfanew) else {
            return 0;
        };
        if !readable(nt, 0x54) || *(nt as *const u32) != 0x0000_4550 {
            return 0;
        }
        *((nt + 80) as *const u32) as usize
    }
}

fn parse_nccp(text: &str) -> Vec<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    for line in text.lines() {
        for token in line.split('"') {
            let Some((addr, hex)) = token.split_once('|') else {
                continue;
            };
            let (Ok(addr), Some(bytes)) = (usize::from_str_radix(addr.trim(), 16), hex_bytes(hex))
            else {
                continue;
            };
            out.push((addr, bytes));
        }
    }
    out
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    // Byte-wise, so a multibyte character in a mod file is a parse error, not a panic.
    let s = s.trim().as_bytes();
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }
    s.chunks_exact(2)
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16)?;
            let lo = char::from(pair[1]).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

fn module_base() -> usize {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}

/// Found by walking up from the DLL to the per-game data dir and taking its parent.
fn mods_root() -> Option<PathBuf> {
    const DATA_DIRS: [&str; 3] = ["white_data", "alba_data", "weiss_data"];
    let dir = dll_dir()?;
    let mut p: &std::path::Path = dir.as_path();
    loop {
        if p.file_name()
            .is_some_and(|n| DATA_DIRS.iter().any(|d| n.eq_ignore_ascii_case(d)))
        {
            return p.parent().map(|parent| parent.join("mods"));
        }
        p = p.parent()?;
    }
}

/// The unpacked-mode sites are short conditional jumps once `.text` is decrypted.
const SHORT_JCC: &[u8] = &[
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
];
/// The 1-byte argument pushes ahead of the MessageBox call; `0x90` means already NOPed.
const PUSH_REG: &[u8] = &[0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x90];
/// The XIII-2 build pushes one 5-byte immediate instead of four 1-byte registers.
const PUSH_IMM32: &[u8] = &[0x68, 0x90];
/// A 5-byte `E8 rel32` is refused, because the 6-byte replacement would spill into the next
/// instruction.
const CALL_OPCODES: &[u8] = &[0xFF, 0xB8];
const MORPH_ADDSS: &[u8] = &[0xF3, 0x0F, 0x58, 0x45, 0x14];

#[derive(Clone, Copy)]
enum Expect {
    /// The first byte must be one of these.
    First(&'static [u8]),
    Exact(&'static [u8]),
    /// No recorded original, so only blank or filler bytes are refused.
    Code,
    /// Any prior content is plausible, so log rather than refuse.
    Data,
}

unsafe fn readable(addr: usize, len: usize) -> bool {
    unsafe {
        let mut mbi: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
        let n = VirtualQuery(
            addr as *const c_void,
            &mut mbi,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        );
        if n == 0 || mbi.State != MEM_COMMIT || mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0 {
            return false;
        }
        let end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        addr.checked_add(len).is_some_and(|last| last <= end)
    }
}

/// `None` when the site is not readable.
unsafe fn pre_bytes(addr: usize, len: usize) -> Option<Vec<u8>> {
    unsafe {
        readable(addr, len).then(|| core::slice::from_raw_parts(addr as *const u8, len).to_vec())
    }
}

unsafe fn site_ok(what: &str, addr: usize, len: usize, expect: Expect) -> bool {
    unsafe {
        let Some(pre) = pre_bytes(addr, len) else {
            flog!("  {what} @ {addr:#x}: not readable; skipping");
            return false;
        };
        flog!("  {what} @ {addr:#x}: pre={pre:02X?}");
        if pre.is_empty() {
            return false;
        }
        match expect {
            Expect::Exact(want) if pre != want => {
                flog!("  {what} @ {addr:#x}: expected {want:02X?}; skipping");
                return false;
            }
            Expect::First(allowed) if !allowed.contains(&pre[0]) => {
                flog!("  {what} @ {addr:#x}: unexpected opcode; skipping");
                return false;
            }
            Expect::Data => return true,
            _ => {}
        }
        if pre.iter().all(|&b| b == 0x00) || pre.iter().all(|&b| b == 0xCC) {
            flog!("  {what} @ {addr:#x}: blank/filler bytes, not the expected code; skipping");
            return false;
        }
        true
    }
}

/// `false` means the protection flip was refused and nothing was written. Does no logging, so it
/// is safe to call with threads frozen.
#[must_use]
unsafe fn write_bytes(addr: usize, bytes: &[u8]) -> bool {
    unsafe {
        let mut old = 0u32;
        if VirtualProtect(
            addr as *mut c_void,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old,
        ) == 0
        {
            return false;
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
        let mut restored = 0u32;
        VirtualProtect(addr as *mut c_void, bytes.len(), old, &mut restored);
        true
    }
}

unsafe fn patch(what: &str, addr: usize, bytes: &[u8]) -> bool {
    unsafe {
        let ok = write_bytes(addr, bytes);
        if !ok {
            flog!("  {what} @ {addr:#x}: VirtualProtect refused, NOT patched");
        }
        ok
    }
}

struct HotSite<'a> {
    what: &'a str,
    addr: usize,
    bytes: &'a [u8],
    expect: Expect,
}

/// All-or-nothing: every site verifies before any byte is written, inside one freeze window.
unsafe fn patch_hot_group(group: &str, sites: &[HotSite]) -> bool {
    unsafe {
        for s in sites {
            if !site_ok(s.what, s.addr, s.bytes.len(), s.expect) {
                flog!(
                    "  {group}: {} failed its site check; whole group refused",
                    s.what
                );
                return false;
            }
        }
        let ranges: Vec<(usize, &[u8])> = sites.iter().map(|s| (s.addr, s.bytes)).collect();
        freeze_and_write(group, &ranges)
    }
}

unsafe fn patch_hot(what: &str, addr: usize, bytes: &[u8]) -> bool {
    unsafe { freeze_and_write(what, &[(addr, bytes)]) }
}

/// Nothing between suspend and resume may allocate or log, since a frozen thread can hold the
/// heap lock.
unsafe fn freeze_and_write(what: &str, ranges: &[(usize, &[u8])]) -> bool {
    unsafe {
        let mut old_prots: Vec<u32> = Vec::with_capacity(ranges.len());
        for &(addr, bytes) in ranges {
            let mut old = 0u32;
            if VirtualProtect(
                addr as *mut c_void,
                bytes.len(),
                PAGE_EXECUTE_READWRITE,
                &mut old,
            ) == 0
            {
                restore_protections(ranges, &old_prots);
                flog!("  {what} @ {addr:#x}: VirtualProtect refused, nothing written");
                return false;
            }
            old_prots.push(old);
        }
        let mut refusal = "";
        let mut written = false;
        for _ in 0..8 {
            let frozen = match suspend_other_threads() {
                Ok(f) => f,
                Err(e) => {
                    refusal = e;
                    break;
                }
            };
            if any_eip_inside(&frozen, ranges) {
                resume_and_close(&frozen);
                refusal = "a thread keeps executing a patched range";
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            for &(addr, bytes) in ranges {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
                FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, bytes.len());
            }
            resume_and_close(&frozen);
            written = true;
            break;
        }
        restore_protections(ranges, &old_prots);
        if !written {
            flog!("  {what}: {refusal}, NOT patched");
        }
        written
    }
}

unsafe fn restore_protections(ranges: &[(usize, &[u8])], old: &[u32]) {
    unsafe {
        for (&(addr, bytes), &prot) in ranges.iter().zip(old) {
            let mut tmp = 0u32;
            VirtualProtect(addr as *mut c_void, bytes.len(), prot, &mut tmp);
        }
    }
}

unsafe fn resume_and_close(frozen: &[HANDLE]) {
    unsafe {
        for &h in frozen {
            ResumeThread(h);
            CloseHandle(h);
        }
    }
}

/// An unreadable thread context counts as inside, since it cannot be proven safe to resume.
unsafe fn any_eip_inside(frozen: &[HANDLE], ranges: &[(usize, &[u8])]) -> bool {
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            CONTEXT, CONTEXT_CONTROL_X86, GetThreadContext,
        };
        frozen.iter().any(|&h| {
            let mut ctx: CONTEXT = core::mem::zeroed();
            ctx.ContextFlags = CONTEXT_CONTROL_X86;
            for _ in 0..4 {
                if GetThreadContext(h, &mut ctx) != 0 {
                    let eip = ctx.Eip as usize;
                    return ranges
                        .iter()
                        .any(|&(addr, bytes)| (addr..addr + bytes.len()).contains(&eip));
                }
            }
            true
        })
    }
}

/// Re-snapshots until a pass adds nothing, so a thread spawned mid-freeze cannot slip through.
unsafe fn suspend_other_threads() -> Result<Vec<HANDLE>, &'static str> {
    unsafe {
        const MAX_THREADS: usize = 1024;
        // Sized up front, because pushing must not allocate while threads are frozen.
        let mut seen: Vec<u32> = Vec::with_capacity(MAX_THREADS);
        let mut frozen: Vec<HANDLE> = Vec::with_capacity(MAX_THREADS);
        let (pid, self_tid) = (GetCurrentProcessId(), GetCurrentThreadId());
        for _ in 0..8 {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap.is_null() || snap == INVALID_HANDLE_VALUE {
                resume_and_close(&frozen);
                return Err("thread snapshot failed");
            }
            let mut grew = false;
            let mut te: THREADENTRY32 = core::mem::zeroed();
            te.dwSize = core::mem::size_of::<THREADENTRY32>() as u32;
            let mut more = Thread32First(snap, &mut te);
            while more != 0 {
                let (owner, tid) = (te.th32OwnerProcessID, te.th32ThreadID);
                more = Thread32Next(snap, &mut te);
                if owner != pid || tid == self_tid || seen.contains(&tid) {
                    continue;
                }
                if seen.len() == MAX_THREADS {
                    CloseHandle(snap);
                    resume_and_close(&frozen);
                    return Err("too many threads to freeze");
                }
                seen.push(tid);
                grew = true;
                let h = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, 0, tid);
                if h.is_null() {
                    continue;
                }
                if SuspendThread(h) == u32::MAX {
                    CloseHandle(h);
                    CloseHandle(snap);
                    resume_and_close(&frozen);
                    return Err("SuspendThread failed");
                }
                frozen.push(h);
            }
            CloseHandle(snap);
            if !grew {
                return Ok(frozen);
            }
        }
        resume_and_close(&frozen);
        Err("thread set never stopped growing")
    }
}

/// `false` means the site looked wrong, or the write failed.
unsafe fn patch_site(what: &str, addr: usize, bytes: &[u8], expect: Expect) -> bool {
    unsafe { site_ok(what, addr, bytes.len(), expect) && patch(what, addr, bytes) }
}

unsafe fn patch_site_hot(what: &str, addr: usize, bytes: &[u8], expect: Expect) -> bool {
    unsafe { site_ok(what, addr, bytes.len(), expect) && patch_hot(what, addr, bytes) }
}

unsafe fn nop_site_hot(what: &str, addr: usize, len: usize, expect: Expect) -> bool {
    unsafe { site_ok(what, addr, len, expect) && nop_hot(what, addr, len) }
}

unsafe fn nop_hot(what: &str, addr: usize, len: usize) -> bool {
    unsafe {
        const NOPS: [u8; 32] = [0x90; 32];
        match NOPS.get(..len) {
            Some(filler) => patch_hot(what, addr, filler),
            None => {
                flog!("  {what} @ {addr:#x}: {len}-byte NOP too long; skipping");
                false
            }
        }
    }
}

/// See [`FacialOffsets`] for what this is correcting.
unsafe fn install_facial_anim_fix(base: usize, f: &FacialOffsets) {
    unsafe {
        let site = base + f.morph_advance;
        let ret = base + f.morph_ret;
        if !site_ok(
            "facial morph step",
            site,
            MORPH_ADDSS.len(),
            Expect::Exact(MORPH_ADDSS),
        ) {
            return;
        }
        let delta = (base + f.frame_delta) as u32;
        let c30 = (base + f.const_30f) as u32;
        let c300k = (base + f.const_300000f) as u32;

        // xmm1 is free as scratch: the original only used xmm0.
        let mut code: Vec<u8> = Vec::with_capacity(40);
        code.extend_from_slice(&[0xF3, 0x0F, 0x2A, 0x0D]);
        code.extend_from_slice(&delta.to_le_bytes());
        code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0x0D]);
        code.extend_from_slice(&c30.to_le_bytes());
        code.extend_from_slice(&[0xF3, 0x0F, 0x5E, 0x0D]);
        code.extend_from_slice(&c300k.to_le_bytes());
        code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0x4D, 0x14]);
        code.extend_from_slice(&[0xF3, 0x0F, 0x58, 0xC1]);

        // Placed just above the module so both rel32 jumps stay in range; a null-hint allocation in
        // a LAA process could land more than 2GB away.
        let cave = alloc_rwx_near(base + 0x0300_0000, code.len() + 5);
        if cave == 0 {
            flog!("  facial_anim_fix: cave alloc failed, skipping");
            return;
        }

        let jmp_at = cave + code.len();
        let rel_ret = (ret as i64 - (jmp_at as i64 + 5)) as i32;
        code.push(0xE9);
        code.extend_from_slice(&rel_ret.to_le_bytes());
        core::ptr::copy_nonoverlapping(code.as_ptr(), cave as *mut u8, code.len());
        FlushInstructionCache(GetCurrentProcess(), cave as *const c_void, code.len());
        let mut old = 0u32;
        if VirtualProtect(cave as *mut c_void, code.len(), PAGE_EXECUTE_READ, &mut old) == 0 {
            flog!("  facial_anim_fix: cave stays writable (VirtualProtect refused)");
        }

        // The 5-byte jump exactly fills the instruction it replaces.
        let rel_site = (cave as i64 - (site as i64 + 5)) as i32;
        let mut jmp = [0xE9u8, 0, 0, 0, 0];
        jmp[1..].copy_from_slice(&rel_site.to_le_bytes());
        if patch_hot("facial morph step", site, &jmp) {
            flog!("  facial_anim_fix: cave @ {cave:#x} (site {site:#x} -> jmp)");
        }
    }
}

/// Scans up in 64KB steps from `hint` to stay within rel32 range, falling back to anywhere if
/// that window is full.
unsafe fn alloc_rwx_near(hint: usize, size: usize) -> usize {
    unsafe {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, VirtualAlloc};
        let mut probe = hint & !0xFFFF;
        for _ in 0..0x1000 {
            let p = VirtualAlloc(
                probe as *const c_void,
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            ) as usize;
            if p != 0 {
                return p;
            }
            probe += 0x10000;
        }
        VirtualAlloc(
            core::ptr::null(),
            size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        ) as usize
    }
}

// The init-time patches must land after SteamStub decrypts `.text` but before the file-system
// code reads them, and that window is narrow: the normal window-triggered pass is too late, and
// `DllMain` too early to write into still-encrypted code. Hence both a kernel32 detour, which the
// CRT's own init calls first, and a polling thread, either of which may win the race.

static ORIG_GST: AtomicUsize = AtomicUsize::new(0);
static ORIG_QPC: AtomicUsize = AtomicUsize::new(0);
/// Kept so the detours can be retired once they have done their job.
static GST_TARGET: AtomicUsize = AtomicUsize::new(0);
static QPC_TARGET: AtomicUsize = AtomicUsize::new(0);
static EARLY_DONE: AtomicBool = AtomicBool::new(false);
/// Claimed by the one thread currently inside [`try_early_patches`].
static EARLY_BUSY: AtomicBool = AtomicBool::new(false);
/// 0 unknown, 1 on, 2 off; set by `run` once the config is loaded.
static UNPACKED_FLAG: AtomicU8 = AtomicU8::new(0);
/// 0 unknown, 1 on, 2 off; set by `run` once the config is loaded.
static DEBUG_FLAG: AtomicU8 = AtomicU8::new(0);

/// Call from `DllMain`, so the detour is in place before the game's entry point runs.
pub fn install_early_hooks() {
    use minhook::MinHook;
    unsafe {
        hook_kernel32(
            c"GetSystemTimeAsFileTime",
            gst_detour as _,
            &GST_TARGET,
            &ORIG_GST,
        );
        hook_kernel32(
            c"QueryPerformanceCounter",
            qpc_detour as _,
            &QPC_TARGET,
            &ORIG_QPC,
        );
        let _ = MinHook::enable_all_hooks();
    }
    flog!(
        "early: CRT-init hooks installed (gst tramp={:#x}, qpc tramp={:#x})",
        ORIG_GST.load(Ordering::Acquire),
        ORIG_QPC.load(Ordering::Acquire)
    );
}

/// Resolves the target itself, because MinHook needs that address to disable the hook again and
/// `create_hook_api` does not hand it back.
unsafe fn hook_kernel32(
    name: &core::ffi::CStr,
    detour: *mut c_void,
    target_cell: &AtomicUsize,
    orig: &AtomicUsize,
) {
    unsafe {
        use minhook::MinHook;
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        let k32 = GetModuleHandleA(c"kernel32.dll".as_ptr() as *const u8);
        let target = if k32.is_null() {
            None
        } else {
            GetProcAddress(k32, name.as_ptr() as *const u8)
        };
        let Some(target) = target else {
            flog!("early: {name:?} not found in kernel32");
            return;
        };
        let target = target as *mut c_void;
        match MinHook::create_hook(target, detour) {
            Ok(t) => {
                orig.store(t as usize, Ordering::Release);
                target_cell.store(target as usize, Ordering::Release);
            }
            Err(e) => flog!("early: {name:?} hook failed: {e:?}"),
        }
    }
}

/// Never call from inside a detour: MinHook freezes threads and rewrites their instruction
/// pointers.
fn disable_early_hooks() {
    use minhook::MinHook;
    for (name, cell) in [
        ("GetSystemTimeAsFileTime", &GST_TARGET),
        ("QueryPerformanceCounter", &QPC_TARGET),
    ] {
        let target = cell.swap(0, Ordering::AcqRel);
        if target == 0 {
            continue;
        }
        match unsafe { MinHook::disable_hook(target as *mut c_void) } {
            Ok(()) => flog!("early: {name} detour disabled"),
            Err(e) => flog!("early: {name} disable failed: {e:?}"),
        }
    }
}

type GstFn = unsafe extern "system" fn(*mut c_void);
type QpcFn = unsafe extern "system" fn(*mut c_void) -> i32;

unsafe extern "system" fn gst_detour(p: *mut c_void) {
    unsafe {
        try_early_patches();
        let o = ORIG_GST.load(Ordering::Acquire);
        if o != 0 {
            let f: GstFn = core::mem::transmute(o);
            f(p);
        }
    }
}

unsafe extern "system" fn qpc_detour(p: *mut c_void) -> i32 {
    unsafe {
        try_early_patches();
        let o = ORIG_QPC.load(Ordering::Acquire);
        if o != 0 {
            let f: QpcFn = core::mem::transmute(o);
            f(p)
        } else {
            1
        }
    }
}

/// On its own thread so it cannot delay window setup, then drops to a slow poll purely to retire
/// the detours.
fn early_patch_poll() {
    let start = std::time::Instant::now();
    while !EARLY_DONE.load(Ordering::Acquire) && start.elapsed().as_secs() < 2 {
        try_early_patches();
        std::hint::spin_loop();
    }
    for _ in 0..600 {
        if EARLY_DONE.load(Ordering::Acquire) {
            disable_early_hooks();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        try_early_patches();
    }
    flog!("early: patches never landed; leaving the CRT-init detours in place");
}

/// The `unpacked` sites double as the decryption probe: each byte only reads as a short `Jcc`
/// once decrypted, and the `debug` offsets share their code page. A disabled patch still probes,
/// read-only. One-shot and idempotent.
fn try_early_patches() {
    if EARLY_DONE.load(Ordering::Acquire) {
        return;
    }
    // Three callers race here, and only the claimant patches.
    if EARLY_BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let done = early_patches_once();
    if done {
        EARLY_DONE.store(true, Ordering::Release);
    }
    EARLY_BUSY.store(false, Ordering::Release);
}

/// `true` once there is nothing left to do, whether patched or not applicable.
fn early_patches_once() -> bool {
    let unpacked = UNPACKED_FLAG.load(Ordering::Acquire);
    let debug = DEBUG_FLAG.load(Ordering::Acquire);
    if unpacked == 0 || debug == 0 {
        return false;
    }
    let (want_unpacked, want_debug) = (unpacked == 1, debug == 1);
    let Some(o) = offsets(detect_game()) else {
        return true;
    };
    // With nothing to do early, or no site to probe decryption with, leave it to `apply_fixes`.
    if (!want_unpacked && !want_debug) || o.unpacked.is_empty() {
        return true;
    }
    let base = module_base();
    if base == 0 {
        return false;
    }
    let mut decrypted = true;
    let mut failed_writes = 0usize;
    for &(off, target) in o.unpacked {
        let b = unsafe { *((base + off) as *const u8) };
        if (0x70..=0x7F).contains(&b) {
            if want_unpacked && !unsafe { write_bytes(base + off, &[target]) } {
                failed_writes += 1;
            }
        } else if b != target {
            decrypted = false; // still encrypted / not yet decrypted
        }
    }
    if !decrypted {
        return false;
    }
    if want_debug {
        for &(off, bytes) in o.debug {
            if !unsafe { write_bytes(base + off, bytes) } {
                failed_writes += 1;
            }
        }
    }
    if failed_writes == 0 {
        flog!(
            "early: pre-file-init patches applied (unpacked={} debug={})",
            want_unpacked,
            want_debug
        );
    } else {
        flog!(
            "early: pre-file-init patches: {failed_writes} write(s) refused by VirtualProtect (unpacked={} debug={})",
            want_unpacked,
            want_debug
        );
    }
    true
}
