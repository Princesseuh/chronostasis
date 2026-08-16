use std::io::Cursor;
use std::path::Path;

use ff13::formats::scd;

#[derive(Default)]
pub struct AudioPlayer {
    path: String,
    /// Kept even on failure, so a bad file is not re-decoded every frame.
    loaded_for: Option<std::path::PathBuf>,
    info: Option<String>,
    status: Option<String>,
    audio: Option<(String, Vec<u8>)>,
    // Opened once for the player's lifetime: re-probing ALSA on every press would re-spam its
    // plugin-chain errors, and the stream has to outlive its sinks anyway.
    out: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
}

impl AudioPlayer {
    pub fn invalidate(&mut self) {
        self.loaded_for = None;
    }
}

pub fn open_path(player: &mut AudioPlayer, path: &Path) {
    if player.loaded_for.as_deref() == Some(path) {
        return;
    }
    player.loaded_for = Some(path.to_path_buf());
    open(player, path);
}

pub fn preview(player: &mut AudioPlayer, ui: &mut egui::Ui) {
    ui.label(
        Path::new(&player.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );

    if let Some(info) = &player.info {
        ui.label(info);
    }

    if player.audio.is_some() {
        ui.separator();
        ui.horizontal(|ui| {
            let playing = player.sink.as_ref().is_some_and(|s| !s.empty());
            if ui.button("▶ Play").clicked() {
                play(player);
            }
            if ui
                .add_enabled(playing, egui::Button::new("⏸ Pause"))
                .clicked()
                && let Some(s) = &player.sink
            {
                s.pause();
            }
            if ui
                .add_enabled(playing, egui::Button::new("⏵ Resume"))
                .clicked()
                && let Some(s) = &player.sink
            {
                s.play();
            }
            if ui.button("⏹ Stop").clicked() {
                stop(player);
            }
            if ui.button("Extract…").clicked() {
                extract(player);
            }
            if ui.button("Replace from ogg/wav…").clicked() {
                replace(player);
            }
        });
        // Only while playing, and only a few times a second, to refresh the button states.
        if player
            .sink
            .as_ref()
            .is_some_and(|s| !s.empty() && !s.is_paused())
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    if let Some(status) = &player.status {
        ui.colored_label(egui::Color32::from_rgb(210, 180, 90), status);
    }
}

fn open(player: &mut AudioPlayer, path: &std::path::Path) {
    stop(player);
    player.path = path.display().to_string();
    player.info = None;
    player.audio = None;
    player.status = None;

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            player.status = Some(format!("read: {e}"));
            return;
        }
    };
    if let Ok(i) = scd::info(&bytes) {
        let codec = match i.codec {
            6 => "Vorbis".to_string(),
            12 => "MS-ADPCM".to_string(),
            other => format!("codec {other}"),
        };
        player.info = Some(format!(
            "{codec}  ·  {} Hz  ·  {} ch  ·  {} KiB",
            i.samplerate,
            i.channels,
            i.stream_size / 1024
        ));
    }
    match scd::extract(&bytes) {
        Ok((ext, audio)) => player.audio = Some((ext.to_string(), audio)),
        Err(e) => player.status = Some(format!("decode: {e}")),
    }
}

fn play(player: &mut AudioPlayer) {
    if !ensure_output(player) {
        return;
    }
    let Some((_, bytes)) = &player.audio else {
        return;
    };
    let bytes = bytes.clone();
    let Some((_, handle)) = &player.out else {
        return;
    };
    match make_sink(handle, bytes) {
        Ok(sink) => {
            player.sink = Some(sink);
            player.status = None;
        }
        Err(e) => player.status = Some(format!("playback: {e}")),
    }
}

fn ensure_output(player: &mut AudioPlayer) -> bool {
    if player.out.is_some() {
        return true;
    }
    match open_output() {
        Ok(pair) => {
            player.out = Some(pair);
            true
        }
        Err(e) => {
            player.status = Some(format!("no audio output device: {e}"));
            false
        }
    }
}

/// By hand, because `try_default` falls through ALSA's plugin chain to the dead `jack` device on
/// a PipeWire or Pulse box, giving no sound at all.
fn open_output() -> anyhow::Result<(rodio::OutputStream, rodio::OutputStreamHandle)> {
    silence_stderr(open_output_inner)
}

/// ALSA enumeration probes every PCM, and those C libraries print failures straight to fd 2, past
/// any handler. This swallows that one-time burst without muting the rest of the app.
#[cfg(unix)]
fn silence_stderr<T>(f: impl FnOnce() -> T) -> T {
    let devnull = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY) };
    if devnull < 0 {
        return f();
    }
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    unsafe {
        libc::dup2(devnull, libc::STDERR_FILENO);
    }
    let out = f();
    unsafe {
        libc::dup2(saved, libc::STDERR_FILENO);
        libc::close(saved);
        libc::close(devnull);
    }
    out
}

#[cfg(not(unix))]
fn silence_stderr<T>(f: impl FnOnce() -> T) -> T {
    f()
}

fn open_output_inner() -> anyhow::Result<(rodio::OutputStream, rodio::OutputStreamHandle)> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};

    let host = rodio::cpal::default_host();
    let devices: Vec<_> = host
        .output_devices()
        .map(|it| it.collect())
        .unwrap_or_default();
    let name_of = |d: &rodio::cpal::Device| d.name().unwrap_or_default().to_ascii_lowercase();
    let blocked = |n: &str| ["jack", "oss", "null"].iter().any(|b| n.contains(b));

    let preferred = ["pipewire", "pulse", "default"];
    let mut order: Vec<&rodio::cpal::Device> = Vec::new();
    for want in preferred {
        order.extend(devices.iter().filter(|d| name_of(d) == want));
    }
    for d in &devices {
        let n = name_of(d);
        if !blocked(&n) && !preferred.contains(&n.as_str()) {
            order.push(d);
        }
    }

    let mut last_err = None;
    for dev in order {
        match rodio::OutputStream::try_from_device(dev) {
            Ok(pair) => return Ok(pair),
            Err(e) => last_err = Some(e),
        }
    }
    Err(anyhow::anyhow!(
        "no usable audio output device{}",
        last_err
            .map(|e| format!(" (last error: {e})"))
            .unwrap_or_default()
    ))
}

fn make_sink(handle: &rodio::OutputStreamHandle, bytes: Vec<u8>) -> anyhow::Result<rodio::Sink> {
    let sink = rodio::Sink::try_new(handle)?;
    sink.append(rodio::Decoder::new(Cursor::new(bytes))?);
    sink.play();
    Ok(sink)
}

fn stop(player: &mut AudioPlayer) {
    player.sink = None;
}

fn extract(player: &mut AudioPlayer) {
    let Some((ext, bytes)) = &player.audio else {
        return;
    };
    let stem = std::path::Path::new(&player.path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    if let Some(p) = rfd::FileDialog::new()
        .set_file_name(format!("{stem}.{ext}"))
        .add_filter(ext.as_str(), &[ext.as_str()])
        .save_file()
    {
        player.status = Some(match std::fs::write(&p, bytes) {
            Ok(()) => format!("wrote {}", p.display()),
            Err(e) => format!("write failed: {e}"),
        });
    }
}

fn replace(player: &mut AudioPlayer) {
    let Some(audio) = rfd::FileDialog::new()
        .add_filter("audio", &["ogg", "wav"])
        .pick_file()
    else {
        return;
    };
    player.status = Some(do_replace(&player.path, &audio).unwrap_or_else(|e| format!("{e:#}")));
    let path = player.path.clone();
    open(player, std::path::Path::new(&path));
}

fn do_replace(scd_path: &str, audio_path: &std::path::Path) -> anyhow::Result<String> {
    let scd_bytes = std::fs::read(scd_path)?;
    let audio_bytes = std::fs::read(audio_path)?;
    let new_scd = scd::replace(&scd_bytes, &audio_bytes)?;
    let bak = std::path::Path::new(scd_path).with_extension("scd.bak");
    if !bak.exists() {
        std::fs::copy(scd_path, &bak)?;
    }
    std::fs::write(scd_path, new_scd)?;
    Ok("replaced SCD stream".into())
}
