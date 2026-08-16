use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use ff13::formats::{imgb, trb::Trb};

use super::ext_lower;

const IMAGE_EXTS: &[&str] = &["dds", "png", "jpg", "jpeg", "gif", "bmp", "tga", "webp"];

type LoadResult = (Vec<LoadedTex>, Option<String>, String);

#[derive(Default)]
pub struct TextureView {
    header_path: String,
    imgb_path: String,
    /// A loose image file, with no GTEX header or `.imgb`.
    single_image: bool,
    single_label: String,
    loaded: Vec<LoadedTex>,
    selected: usize,
    status: Option<String>,
    loaded_for: Option<PathBuf>,
    /// Reading and decoding a DDS can take seconds.
    loading: Option<Receiver<LoadResult>>,
    channel: Channel,
    /// Keyed by `(selected, channel)`.
    chan_handle: Option<egui::TextureHandle>,
    chan_key: Option<(usize, Channel)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Channel {
    #[default]
    Rgba,
    Rgb,
    R,
    G,
    B,
    A,
}

struct LoadedTex {
    index: usize,
    width: u16,
    height: u16,
    format: u8,
    handle: egui::TextureHandle,
    dds: Vec<u8>,
    /// Kept so channel views rebuild without re-decoding.
    rgba: Vec<u8>,
    /// `None` for loose images and non-TRB headers.
    name: Option<String>,
    /// Inferred, since the format stores none; see `colorspace_for`.
    colorspace: String,
    /// Shown raw for inspection; possibly a colour-space flag.
    unk8: Option<u8>,
}

impl TextureView {
    pub fn invalidate(&mut self) {
        self.loaded_for = None;
    }
}

/// Accepts either side of a header/imgb pair, or a loose image.
pub fn open_path(view: &mut TextureView, path: &Path, ctx: &egui::Context) {
    if view.loaded_for.as_deref() == Some(path) {
        return;
    }
    resolve_pair(view, path);
    start_load(view, ctx);
    view.loaded_for = Some(path.to_path_buf());
}

fn resolve_pair(view: &mut TextureView, path: &Path) {
    view.single_image = false;
    let ext = ext_lower(path);
    if is_image_ext(ext.as_deref()) {
        view.single_image = true;
        view.header_path = path.display().to_string();
        view.imgb_path.clear();
    } else if ext.as_deref() == Some("imgb") {
        view.imgb_path = path.display().to_string();
        view.header_path = ["trb", "xgr", "wpd"]
            .iter()
            .map(|e| path.with_extension(e))
            .find(|h| h.is_file())
            .unwrap_or_else(|| path.with_extension("trb"))
            .display()
            .to_string();
    } else {
        view.header_path = path.display().to_string();
        view.imgb_path = path.with_extension("imgb").display().to_string();
    }
}

pub fn preview(view: &mut TextureView, ui: &mut egui::Ui) {
    if let Some(rx) = &view.loading {
        match rx.try_recv() {
            Ok((loaded, status, label)) => {
                view.loaded = loaded;
                view.status = status;
                if !label.is_empty() {
                    view.single_label = label;
                }
                view.loading = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                view.loading = None;
                view.status = Some("load failed".into());
            }
        }
    }

    ui.horizontal(|ui| {
        ui.label(file_name(&view.header_path));
        if ui.small_button("↺").on_hover_text("Reload").clicked() {
            start_load(view, ui.ctx());
        }
    });
    if !view.single_image {
        ui.horizontal(|ui| {
            ui.label("imgb:");
            ui.add(egui::TextEdit::singleline(&mut view.imgb_path).desired_width(360.0));
            if ui.small_button("Browse…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("imgb", &["imgb"])
                    .pick_file()
            {
                view.imgb_path = p.display().to_string();
                start_load(view, ui.ctx());
            }
        });
    }

    if view.loading.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("decoding textures…");
        });
        return;
    }
    if let Some(status) = &view.status {
        ui.colored_label(egui::Color32::from_rgb(210, 180, 90), status);
    }
    if view.loaded.is_empty() {
        return;
    }

    ui.separator();
    if !view.single_image {
        ui.label(format!("{} texture(s)", view.loaded.len()));
        let mut clicked = None;
        egui::ScrollArea::horizontal()
            .id_salt("tex_thumbs")
            .max_height(110.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..view.loaded.len() {
                        let t = &view.loaded[i];
                        let img = egui::Image::new(&t.handle)
                            .fit_to_exact_size(egui::vec2(90.0, 90.0))
                            .maintain_aspect_ratio(true);
                        if ui
                            .add(egui::ImageButton::new(img).selected(i == view.selected))
                            .on_hover_text(format!("#{}  {}×{}", t.index, t.width, t.height))
                            .clicked()
                        {
                            clicked = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            view.selected = i;
        }
    }

    let sel = view.selected.min(view.loaded.len() - 1);
    view.selected = sel;

    let (index, width, height, format, colorspace, name, unk8) = {
        let t = &view.loaded[sel];
        (
            t.index,
            t.width,
            t.height,
            t.format,
            t.colorspace.clone(),
            t.name.clone(),
            t.unk8,
        )
    };
    ui.horizontal(|ui| {
        if !view.single_image {
            match &name {
                Some(n) => {
                    ui.strong(short_name(n)).on_hover_text(n);
                }
                None => {
                    ui.strong(format!("#{index}"));
                }
            }
        }
        ui.label(format!("{width} × {height}"));
        ui.label(if view.single_image {
            view.single_label.clone()
        } else {
            format_name(format).to_string()
        });
    });
    ui.horizontal(|ui| {
        ui.label("color space:");
        ui.colored_label(egui::Color32::from_rgb(150, 180, 210), &colorspace)
            .on_hover_text(
                "Inferred from the texture's name/role; FFXIII textures don't store a color space.",
            );
        if let Some(flag) = unk8 {
            ui.weak(format!("· GTEX +8 = 0x{flag:02X}")).on_hover_text(
                "Raw GTEX byte +8: an unconfirmed candidate for an sRGB/color-space flag. \
                 Compare a known diffuse against a normal map to see whether it varies.",
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label("channels:");
        for (ch, lbl) in [
            (Channel::Rgba, "RGBA"),
            (Channel::Rgb, "RGB"),
            (Channel::R, "R"),
            (Channel::G, "G"),
            (Channel::B, "B"),
            (Channel::A, "A"),
        ] {
            ui.selectable_value(&mut view.channel, ch, lbl);
        }
    });

    // Only header and imgb textures can be extracted or replaced.
    if !view.single_image {
        ui.horizontal(|ui| {
            if ui.button("Extract this DDS…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .set_file_name(format!("tex_{index}.dds"))
                    .add_filter("DDS", &["dds"])
                    .save_file()
            {
                let dds = view.loaded[sel].dds.clone();
                view.status = Some(match std::fs::write(&p, dds) {
                    Ok(()) => format!("wrote {}", p.display()),
                    Err(e) => format!("write failed: {e}"),
                });
            }
            if ui.button("Extract all…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                view.status = Some(extract_all(view, &dir));
            }
            if ui.button("Replace from DDS…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("DDS", &["dds"])
                    .pick_file()
            {
                let status = replace(view, sel, &p);
                start_load(view, ui.ctx());
                view.status = Some(status);
            }
        });
    }

    let tex = channel_texture(view, sel, ui.ctx());
    egui::ScrollArea::both()
        .id_salt("tex_preview")
        .show(ui, |ui| {
            ui.add(
                egui::Image::new(tex)
                    .max_height(420.0)
                    .maintain_aspect_ratio(true)
                    .bg_fill(egui::Color32::from_gray(30)),
            );
        });
}

/// egui's texture manager is thread-safe, so the handles can be made off-thread.
fn start_load(view: &mut TextureView, ctx: &egui::Context) {
    view.loaded.clear();
    view.selected = 0;
    view.status = None;
    view.chan_handle = None;
    view.chan_key = None;

    let (tx, rx) = std::sync::mpsc::channel();
    view.loading = Some(rx);
    let single = view.single_image;
    let header_path = view.header_path.clone();
    let imgb_path = view.imgb_path.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = if single {
            decode_single_image(&header_path, &ctx)
        } else {
            decode_container(&header_path, &imgb_path, &ctx)
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    });
}

fn decode_container(header_path: &str, imgb_path: &str, ctx: &egui::Context) -> LoadResult {
    let mut loaded = Vec::new();
    let header = match std::fs::read(header_path) {
        Ok(b) => b,
        Err(e) => {
            return (
                loaded,
                Some(format!("could not read header: {e}")),
                String::new(),
            );
        }
    };
    let imgb = std::fs::read(imgb_path).unwrap_or_default();

    let mut status = None;
    match imgb::extract(&header, &imgb) {
        Ok(texs) if !texs.is_empty() => {
            let names = texture_names(&header);
            let mut failed = 0;
            for (i, t) in texs.iter().enumerate() {
                match dds_to_rgba(&t.dds) {
                    Ok((size, rgba)) => {
                        let ci = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                        let handle = ctx.load_texture(
                            format!("ff13tex_{i}"),
                            ci,
                            egui::TextureOptions::LINEAR,
                        );
                        let name = name_for_offset(&names, t.gtex_offset);
                        let colorspace = colorspace_for(name.as_deref(), None, &t.dds);
                        loaded.push(LoadedTex {
                            index: i,
                            width: t.width,
                            height: t.height,
                            format: t.format,
                            handle,
                            dds: t.dds.clone(),
                            rgba,
                            name,
                            colorspace,
                            unk8: Some(t.unk8),
                        });
                    }
                    Err(_) => failed += 1,
                }
            }
            if failed > 0 {
                status = Some(format!("{failed} texture(s) could not be previewed"));
            }
        }
        _ => status = Some(explain_empty(&header, &imgb, imgb_path)),
    }
    (loaded, status, String::new())
}

fn decode_single_image(path: &str, ctx: &egui::Context) -> LoadResult {
    let is_dds = ext_lower(Path::new(path)).as_deref() == Some("dds");
    let label = if is_dds {
        "DDS".to_string()
    } else {
        ext_lower(Path::new(path))
            .map(|e| e.to_uppercase())
            .unwrap_or_else(|| "image".into())
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("could not read image: {e}")),
                label,
            );
        }
    };
    let decoded = if is_dds {
        dds_to_rgba(&bytes)
    } else {
        image_to_rgba(&bytes)
    };
    match decoded {
        Ok((size, rgba)) => {
            let (width, height) = (size[0] as u16, size[1] as u16);
            let ci = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
            let handle = ctx.load_texture("ff13img", ci, egui::TextureOptions::LINEAR);
            let colorspace = colorspace_for(None, Some(&label), &bytes);
            let tex = LoadedTex {
                index: 0,
                width,
                height,
                format: u8::MAX,
                handle,
                dds: bytes,
                rgba,
                name: None,
                colorspace,
                unk8: None,
            };
            (vec![tex], None, label)
        }
        Err(e) => (
            Vec::new(),
            Some(format!("could not decode image: {e}")),
            label,
        ),
    }
}

fn explain_empty(header: &[u8], imgb: &[u8], imgb_path: &str) -> String {
    let gtex = imgb::find_gtex(header).len();
    if gtex == 0 {
        "no GTEX textures here; this looks like a model or non-texture container".into()
    } else if imgb.is_empty() {
        format!("{gtex} texture header(s) found, but no pixel data; expected {imgb_path}")
    } else {
        format!("{gtex} texture header(s) found, but none fit this .imgb (wrong .imgb file?)")
    }
}

fn extract_all(view: &TextureView, dir: &Path) -> String {
    let mut n = 0;
    for t in &view.loaded {
        if std::fs::write(dir.join(format!("tex_{}.dds", t.index)), &t.dds).is_ok() {
            n += 1;
        }
    }
    format!("wrote {n} DDS file(s) to {}", dir.display())
}

/// In place, so the DDS must match dimensions and format. Backs up to `.imgb.bak` once.
fn replace(view: &TextureView, sel: usize, dds_path: &Path) -> String {
    let header = match std::fs::read(&view.header_path) {
        Ok(b) => b,
        Err(e) => return format!("header: {e}"),
    };
    let mut imgb = match std::fs::read(&view.imgb_path) {
        Ok(b) => b,
        Err(e) => return format!("imgb: {e}"),
    };
    let Some(gtex) = imgb::valid_gtex(&header, imgb.len()).into_iter().nth(sel) else {
        return "could not locate this texture in the header".into();
    };
    let dds = match std::fs::read(dds_path) {
        Ok(b) => b,
        Err(e) => return format!("dds: {e}"),
    };
    if let Err(e) = imgb::replace_in_place(&gtex, &mut imgb, &dds) {
        return format!("replace failed (size/format must match): {e}");
    }
    let bak = Path::new(&view.imgb_path).with_extension("imgb.bak");
    if !bak.exists() {
        let _ = std::fs::copy(&view.imgb_path, &bak);
    }
    match std::fs::write(&view.imgb_path, &imgb) {
        Ok(()) => format!("replaced #{sel}"),
        Err(e) => format!("write failed: {e}"),
    }
}

/// Returns the thumbnail and the container's texture count.
pub(super) fn thumb_for(path: &Path) -> Option<(egui::ColorImage, usize)> {
    match ext_lower(path).as_deref() {
        Some("dds") => Some((dds_to_thumb(&std::fs::read(path).ok()?, 96)?, 1)),
        Some(e) if IMAGE_EXTS.contains(&e) => {
            Some((image_to_thumb(&std::fs::read(path).ok()?, 96)?, 1))
        }
        _ => {
            // Counts every texture but only copies the first one's pixels out of the imgb.
            let header = std::fs::read(path).ok()?;
            let imgb = std::fs::read(path.with_extension("imgb")).ok()?;
            let (tex, count) = imgb::first_and_count(&header, &imgb).ok()?;
            Some((dds_to_thumb(&tex.dds, 96)?, count))
        }
    }
}

fn dds_to_rgba(dds: &[u8]) -> anyhow::Result<([usize; 2], Vec<u8>)> {
    let parsed = ddsfile::Dds::read(std::io::Cursor::new(dds))?;
    let image = image_dds::image_from_dds(&parsed, 0)?;
    let size = [image.width() as usize, image.height() as usize];
    Ok((size, image.into_raw()))
}

fn image_to_rgba(bytes: &[u8]) -> anyhow::Result<([usize; 2], Vec<u8>)> {
    let image = image::load_from_memory(bytes)?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok((size, image.into_raw()))
}

fn channel_image(size: [usize; 2], rgba: &[u8], ch: Channel) -> egui::ColorImage {
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        let px = match ch {
            Channel::Rgba => [p[0], p[1], p[2], p[3]],
            Channel::Rgb => [p[0], p[1], p[2], 255],
            Channel::R => [p[0], p[0], p[0], 255],
            Channel::G => [p[1], p[1], p[1], 255],
            Channel::B => [p[2], p[2], p[2], 255],
            Channel::A => [p[3], p[3], p[3], 255],
        };
        out.extend_from_slice(&px);
    }
    egui::ColorImage::from_rgba_unmultiplied(size, &out)
}

/// RGBA reuses the original handle; other channels build and cache a derived image.
fn channel_texture(
    view: &mut TextureView,
    sel: usize,
    ctx: &egui::Context,
) -> egui::load::SizedTexture {
    let t = &view.loaded[sel];
    let size = egui::vec2(t.width as f32, t.height as f32);
    if view.channel == Channel::Rgba {
        return egui::load::SizedTexture::new(t.handle.id(), size);
    }
    let key = (sel, view.channel);
    if view.chan_key != Some(key) {
        let ci = channel_image([t.width as usize, t.height as usize], &t.rgba, view.channel);
        view.chan_handle = Some(ctx.load_texture("ff13tex_chan", ci, egui::TextureOptions::LINEAR));
        view.chan_key = Some(key);
    }
    egui::load::SizedTexture::new(view.chan_handle.as_ref().unwrap().id(), size)
}

/// Empty when the header is not a TRB, which leaves its textures unnamed.
fn texture_names(header: &[u8]) -> Vec<(usize, usize, String)> {
    let Ok(trb) = Trb::parse(header) else {
        return Vec::new();
    };
    let names = trb.resource_names();
    (0..trb.resource_count())
        .filter_map(|i| {
            let (start, end) = trb.resource_abs_span(i)?;
            Some((start, end, names.get(i)?.clone()))
        })
        .collect()
}

fn name_for_offset(spans: &[(usize, usize, String)], off: usize) -> Option<String> {
    spans
        .iter()
        .find(|&&(start, end, _)| off >= start && off < end)
        .map(|(_, _, n)| n.clone())
}

/// `F11\c001C_01.win32` gives `C`.
fn role_letter(name: &str) -> Option<char> {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let base = base.strip_suffix(".win32").unwrap_or(base);
    base.trim_start_matches(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        .chars()
        .next()
}

/// Best-effort: the format carries no colour-space flag, so header textures go by the name's type
/// letter and loose files by their container.
fn colorspace_for(name: Option<&str>, loose_label: Option<&str>, dds: &[u8]) -> String {
    if let Some(letter) = name.and_then(role_letter) {
        return match letter {
            'C' | 'D' => "sRGB · color/diffuse (inferred)",
            'N' => "linear · normal map (inferred)",
            'S' => "linear · specular (inferred)",
            'G' => "linear · mask (inferred)",
            'T' => "sRGB-encoded · tone ramp (inferred)",
            _ => "unknown",
        }
        .to_string();
    }
    match loose_label {
        Some("DDS") => dds_srgb_flag(dds),
        Some(_) => "sRGB (assumed)".to_string(),
        None => "unknown".to_string(),
    }
}

/// Only a DX10 extension header carries one; classic DDS has none.
fn dds_srgb_flag(dds: &[u8]) -> String {
    match ddsfile::Dds::read(std::io::Cursor::new(dds)) {
        Ok(d) => match d.header10 {
            Some(h) if format!("{:?}", h.dxgi_format).contains("Srgb") => "sRGB (DX10 flag)".into(),
            Some(_) => "linear · no sRGB flag (DX10)".into(),
            None => "unknown · no color-space flag".into(),
        },
        Err(_) => "unknown".into(),
    }
}

fn dds_to_thumb(dds: &[u8], box_px: u32) -> Option<egui::ColorImage> {
    let parsed = ddsfile::Dds::read(std::io::Cursor::new(dds)).ok()?;
    Some(rgba_to_thumb(
        image_dds::image_from_dds(&parsed, 0).ok()?,
        box_px,
    ))
}

fn image_to_thumb(bytes: &[u8], box_px: u32) -> Option<egui::ColorImage> {
    Some(rgba_to_thumb(
        image::load_from_memory(bytes).ok()?.to_rgba8(),
        box_px,
    ))
}

fn rgba_to_thumb(img: image::RgbaImage, box_px: u32) -> egui::ColorImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = box_px as f32 / w.max(h) as f32;
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    let thumb = image::imageops::thumbnail(&img, tw, th);
    egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], thumb.as_raw())
}

fn is_image_ext(ext: Option<&str>) -> bool {
    ext.is_some_and(|e| IMAGE_EXTS.contains(&e))
}

/// `F11\c001C_01.win32` gives `c001C_01.win32`.
fn short_name(name: &str) -> String {
    name.rsplit(['\\', '/']).next().unwrap_or(name).to_string()
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn format_name(format: u8) -> &'static str {
    match format {
        3 | 4 => "ARGB (uncompressed)",
        24 => "DXT1",
        25 => "DXT3",
        26 => "DXT5",
        _ => "unknown",
    }
}
