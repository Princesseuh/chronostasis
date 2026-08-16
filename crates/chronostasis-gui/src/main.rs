//! Desktop front-end over the `ff13` operations library.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod job;
mod modder;
mod modpacks;
mod player;

use app::App;

fn main() -> eframe::Result<()> {
    silence_alsa();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 760.0])
            .with_min_inner_size([560.0, 420.0])
            .with_title("Chronostasis"),
        // `POLYGON_MODE_LINE` backs the model viewer's wireframe toggle.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            device_descriptor: std::sync::Arc::new(|adapter| {
                use eframe::egui_wgpu::wgpu;
                let base = if adapter.get_info().backend == wgpu::Backend::Gl {
                    wgpu::Limits::downlevel_webgl2_defaults()
                } else {
                    wgpu::Limits::default()
                };
                wgpu::DeviceDescriptor {
                    label: Some("chronostasis-gui device"),
                    required_features: wgpu::Features::POLYGON_MODE_LINE & adapter.features(),
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: 8192,
                        ..base
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                }
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Chronostasis",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// On PipeWire and PulseAudio systems the ALSA backend walks its whole plugin chain opening the
/// default device, printing each miss straight to stderr even though playback then works.
///
/// The handler is variadic in C, but declaring only its fixed prefix is ABI-compatible here,
/// since none of the arguments are read.
#[cfg(target_os = "linux")]
fn silence_alsa() {
    use std::os::raw::{c_char, c_int};

    type Handler = unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char);
    unsafe extern "C" {
        fn snd_lib_error_set_handler(handler: Handler) -> c_int;
    }
    unsafe extern "C" fn quiet(
        _file: *const c_char,
        _line: c_int,
        _function: *const c_char,
        _err: c_int,
        _fmt: *const c_char,
    ) {
    }
    unsafe { snd_lib_error_set_handler(quiet) };
}

#[cfg(not(target_os = "linux"))]
fn silence_alsa() {}
