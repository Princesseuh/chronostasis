//! The proxy's `chronostasis.ini`, as a structured config every UI can edit.

/// Past the launcher's resolution cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

/// Defaults are the known-good shipped config; only `unpacked_mode`, `debug_mode` and
/// `text_language` are routinely overridden per deploy.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteConfig {
    pub framerate_uncap: bool,
    /// 0 means no cap. Needs `framerate_uncap`.
    pub frame_rate_limit: u32,
    /// Rescales facial morph steps by frame delta so blinks and lip-sync stop running too fast
    /// when uncapped. Skipped on builds with no known offsets.
    pub facial_anim_fix: bool,
    pub controller_scan_fix: bool,
    pub controller_hotplug: bool,
    pub controller_trigger_fix: bool,
    pub device_fixes: bool,
    /// `None` uses the game's chosen size.
    pub render_resolution: Option<Resolution>,
    /// 2-16; 0 and 1 are off.
    pub anisotropic: u32,
    pub experimental_mipmap_fix: bool,
    /// Negative is sharper; `None` leaves the game's value.
    pub lod_bias: Option<f32>,
    pub startup_overlay: bool,
    pub texture_mods: bool,
    /// Off by default: only safe for same-dimension swaps, since a higher-res container resizes
    /// the HUD. Use a `.dds` for HD GUI instead.
    pub unsafe_gui_overlay: bool,
    pub vibration: bool,
    pub vibration_strength: f32,
    pub confirm_exit: bool,
    pub unpacked_mode: bool,
    pub debug_mode: bool,
    /// One of en/fr/de/it/es/jp/cn/kr; `None` is the game default.
    pub text_language: Option<String>,
    pub triple_buffering: bool,
    pub present_interval: i32,
    pub fullscreen_refresh: i32,
    pub multisample: u32,
    pub swap_effect: i32,
    pub borderless: bool,
    pub force_windowed: bool,
    pub always_active: bool,
    pub hide_cursor: bool,
    pub top_most: bool,
}

impl Default for SuiteConfig {
    fn default() -> Self {
        Self {
            framerate_uncap: true,
            frame_rate_limit: 0,
            facial_anim_fix: true,
            controller_scan_fix: true,
            controller_hotplug: true,
            controller_trigger_fix: false,
            device_fixes: true,
            render_resolution: None,
            anisotropic: 16,
            experimental_mipmap_fix: true,
            lod_bias: None,
            startup_overlay: true,
            texture_mods: true,
            unsafe_gui_overlay: false,
            vibration: true,
            vibration_strength: 2.0,
            confirm_exit: true,
            unpacked_mode: false,
            debug_mode: false,
            text_language: None,
            triple_buffering: false,
            present_interval: 0,
            fullscreen_refresh: -1,
            multisample: 0,
            swap_effect: -1,
            borderless: false,
            force_windowed: false,
            always_active: false,
            hide_cursor: false,
            top_most: false,
        }
    }
}

/// Keeps the decimal point, so the ini reads as `2.0` rather than `2`.
fn f(v: f32) -> String {
    format!("{v:?}")
}

impl SuiteConfig {
    /// Unset optional fields render as commented example lines rather than vanishing.
    pub fn render(&self) -> String {
        let render_resolution = match self.render_resolution {
            Some(r) => format!("render_resolution = {}x{}", r.width, r.height),
            None => "# render_resolution = 2560x1440".to_string(),
        };
        let lod_bias = match self.lod_bias {
            Some(v) => format!("lod_bias = {}", f(v)),
            None => "# lod_bias = 0".to_string(),
        };
        let lang = match &self.text_language {
            Some(l) => format!("text_language = {l}"),
            None => "# text_language = en".to_string(),
        };
        format!(
            "# Chronostasis proxy configuration (edit and relaunch)\n\
             framerate_uncap = {framerate_uncap}\n\
             # frame_rate_limit: clean in-engine cap when uncapped (needs framerate_uncap).\n\
             # 0 = no cap. A cap like 60/120/360 avoids wasted frames and the\n\
             # physics/animation jank some cutscenes get when fully uncapped.\n\
             frame_rate_limit = {frame_rate_limit}\n\
             # facial_anim_fix: keep blinking and cutscene lip-sync at the right speed.\n\
             # Vanilla is tuned for 30fps and runs ~5x too fast when uncapped. Leave on\n\
             # unless faces look wrong; skipped on builds it isn't supported for.\n\
             facial_anim_fix = {facial_anim_fix}\n\
             # controller_scan_fix: stop the per-second controller-scan stutter.\n\
             controller_scan_fix = {controller_scan_fix}\n\
             # controller_hotplug: still detect controllers plugged in after launch\n\
             # (re-enables the scan briefly off-thread; no stutter). Needs scan_fix.\n\
             controller_hotplug = {controller_hotplug}\n\
             # controller_trigger_fix: recombine the Xbox One pad's split LT/RT axes\n\
             # into the single Z axis the game expects from a 360 pad (DirectInput).\n\
             # Off by default: only enable if your Xbox One pad's triggers misbehave\n\
             # (e.g. LT/RT share or cancel on one axis). Leave off if they already work\n\
             # -- on it would re-introduce the bug. Only ever touches \"Xbox One\" pads.\n\
             controller_trigger_fix = {controller_trigger_fix}\n\
             # device_fixes: D3D9 vtable hooks for the resolution/scissor/UI fix.\n\
             device_fixes = {device_fixes}\n\
             # render_resolution: force render+backbuffer size past the launcher's cap\n\
             # (experimental, e.g. 2560x1440). Blank = use the game's chosen resolution.\n\
             {render_resolution}\n\
             # anisotropic: force anisotropic texture filtering (2-16; 0/1 = off).\n\
             anisotropic = {anisotropic}\n\
             # experimental_mipmap_fix: give un-mipmapped textures a full auto-generated\n\
             # mip chain so anisotropic filtering stops the shimmer. Pairs with `anisotropic`.\n\
             experimental_mipmap_fix = {experimental_mipmap_fix}\n\
             # lod_bias: override the mip LOD bias. Negative = sharper (more shimmer),\n\
             # 0 = neutral, positive = softer (less shimmer). Blank = leave the game's value.\n\
             {lod_bias}\n\
             # startup_overlay: flash a bright-pink bar in the top-left for a few seconds\n\
             # at launch to confirm the proxy is active.\n\
             startup_overlay = {startup_overlay}\n\
             # texture_mods: LayeredFS texture overrides. Drop files mirroring the\n\
             # white_data tree under <install>/mods (e.g.\n\
             # mods/gui/resident/monster/monster000.dds). GUI textures are swapped at\n\
             # the GPU level so they stay the same on-screen size; other (3D) textures\n\
             # are replaced as files. Needs unpacked_mode.\n\
             texture_mods = {texture_mods}\n\
             # unsafe_gui_overlay: also serve GUI texture containers (.imgb/.xgr) via the raw file\n\
             # overlay. Off by default and deliberately \"unsafe\": only same-dimension swaps work --\n\
             # a higher-res container resizes/corrupts the HUD. For HD GUI, use a .dds instead\n\
             # (hash-swapped at the GPU level). Needs texture_mods + unpacked_mode.\n\
             unsafe_gui_overlay = {unsafe_gui_overlay}\n\
             vibration = {vibration}\n\
             vibration_strength = {vibration_strength}\n\
             # confirm_exit: keep the game's \"Quit game?\" dialog. Set false to remove\n\
             # it (the dialog can hang under DXVK; false exits cleanly instead).\n\
             confirm_exit = {confirm_exit}\n\
             unpacked_mode = {unpacked_mode}\n\
             debug_mode = {debug_mode}\n\
             {lang}\n\
             # Presentation (advanced; DXVK/Proton usually handles these):\n\
             # present_interval: 1 = vsync on, -1 = off, 0 = leave\n\
             triple_buffering = {triple_buffering}\n\
             present_interval = {present_interval}\n\
             fullscreen_refresh = {fullscreen_refresh}\n\
             multisample = {multisample}\n\
             swap_effect = {swap_effect}\n\
             # Window management (native Windows; on Linux the compositor usually handles this):\n\
             borderless = {borderless}\n\
             force_windowed = {force_windowed}\n\
             always_active = {always_active}\n\
             hide_cursor = {hide_cursor}\n\
             top_most = {top_most}\n",
            framerate_uncap = self.framerate_uncap,
            frame_rate_limit = self.frame_rate_limit,
            facial_anim_fix = self.facial_anim_fix,
            controller_scan_fix = self.controller_scan_fix,
            controller_hotplug = self.controller_hotplug,
            controller_trigger_fix = self.controller_trigger_fix,
            device_fixes = self.device_fixes,
            anisotropic = self.anisotropic,
            experimental_mipmap_fix = self.experimental_mipmap_fix,
            startup_overlay = self.startup_overlay,
            texture_mods = self.texture_mods,
            unsafe_gui_overlay = self.unsafe_gui_overlay,
            vibration = self.vibration,
            vibration_strength = f(self.vibration_strength),
            confirm_exit = self.confirm_exit,
            unpacked_mode = self.unpacked_mode,
            debug_mode = self.debug_mode,
            triple_buffering = self.triple_buffering,
            present_interval = self.present_interval,
            fullscreen_refresh = self.fullscreen_refresh,
            multisample = self.multisample,
            swap_effect = self.swap_effect,
            borderless = self.borderless,
            force_windowed = self.force_windowed,
            always_active = self.always_active,
            hide_cursor = self.hide_cursor,
            top_most = self.top_most,
        )
    }

    /// Starts from [`Default`] and overrides any recognized line, ignoring everything else, so a
    /// user's hand-edits survive a round-trip.
    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let (key, val) = (key.trim(), val.trim());
            match key {
                "framerate_uncap" => set_bool(&mut c.framerate_uncap, val),
                "frame_rate_limit" => set_num(&mut c.frame_rate_limit, val),
                "facial_anim_fix" => set_bool(&mut c.facial_anim_fix, val),
                "controller_scan_fix" => set_bool(&mut c.controller_scan_fix, val),
                "controller_hotplug" => set_bool(&mut c.controller_hotplug, val),
                "controller_trigger_fix" => set_bool(&mut c.controller_trigger_fix, val),
                "device_fixes" => set_bool(&mut c.device_fixes, val),
                "render_resolution" => c.render_resolution = parse_resolution(val),
                "anisotropic" => set_num(&mut c.anisotropic, val),
                "experimental_mipmap_fix" => set_bool(&mut c.experimental_mipmap_fix, val),
                "lod_bias" => c.lod_bias = val.parse().ok(),
                "startup_overlay" => set_bool(&mut c.startup_overlay, val),
                "texture_mods" => set_bool(&mut c.texture_mods, val),
                "unsafe_gui_overlay" => set_bool(&mut c.unsafe_gui_overlay, val),
                "vibration" => set_bool(&mut c.vibration, val),
                "vibration_strength" => set_num(&mut c.vibration_strength, val),
                "confirm_exit" => set_bool(&mut c.confirm_exit, val),
                "unpacked_mode" => set_bool(&mut c.unpacked_mode, val),
                "debug_mode" => set_bool(&mut c.debug_mode, val),
                "text_language" => c.text_language = (!val.is_empty()).then(|| val.to_string()),
                "triple_buffering" => set_bool(&mut c.triple_buffering, val),
                "present_interval" => set_num(&mut c.present_interval, val),
                "fullscreen_refresh" => set_num(&mut c.fullscreen_refresh, val),
                "multisample" => set_num(&mut c.multisample, val),
                "swap_effect" => set_num(&mut c.swap_effect, val),
                "borderless" => set_bool(&mut c.borderless, val),
                "force_windowed" => set_bool(&mut c.force_windowed, val),
                "always_active" => set_bool(&mut c.always_active, val),
                "hide_cursor" => set_bool(&mut c.hide_cursor, val),
                "top_most" => set_bool(&mut c.top_most, val),
                _ => {}
            }
        }
        c
    }
}

fn set_bool(field: &mut bool, val: &str) {
    if let Ok(b) = val.parse() {
        *field = b;
    }
}

fn set_num<T: std::str::FromStr>(field: &mut T, val: &str) {
    if let Ok(n) = val.parse() {
        *field = n;
    }
}

fn parse_resolution(val: &str) -> Option<Resolution> {
    let (w, h) = val.split_once(['x', 'X'])?;
    Some(Resolution {
        width: w.trim().parse().ok()?,
        height: h.trim().parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips() {
        let cfg = SuiteConfig::default();
        assert_eq!(SuiteConfig::parse(&cfg.render()), cfg);
    }

    #[test]
    fn edits_survive_round_trip() {
        let cfg = SuiteConfig {
            unpacked_mode: true,
            debug_mode: true,
            text_language: Some("jp".into()),
            render_resolution: Some(Resolution {
                width: 2560,
                height: 1440,
            }),
            lod_bias: Some(-0.5),
            anisotropic: 8,
            frame_rate_limit: 120,
            vibration_strength: 1.5,
            ..Default::default()
        };
        assert_eq!(SuiteConfig::parse(&cfg.render()), cfg);
    }

    #[test]
    fn unknown_and_comment_lines_ignored() {
        let c = SuiteConfig::parse("# a comment\n\nmystery = 7\nanisotropic = 4\n");
        assert_eq!(c.anisotropic, 4);
        assert_eq!(
            c,
            SuiteConfig {
                anisotropic: 4,
                ..Default::default()
            }
        );
    }
}
