use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui_wgpu::{self, wgpu};
use ff13::formats::trb::Trb;
use glam::Vec3;

mod game_shader;
mod gpu;
mod lighting;
mod rig;
#[cfg(test)]
mod tests;

pub(crate) use ff13::formats::model::*;
pub(crate) use gpu::*;
pub(crate) use lighting::*;
pub(crate) use rig::*;

struct UvPage {
    label: String,
    tex: Option<usize>,
    tris: Vec<[[f32; 2]; 3]>,
}

fn build_uv(model: &Model) -> (Vec<UvPage>, Vec<Option<egui::ColorImage>>) {
    let mut images: Vec<Option<egui::ColorImage>> =
        (0..model.textures.len()).map(|_| None).collect();
    let mut pages: Vec<UvPage> = Vec::new();
    let mut page_of: HashMap<Option<usize>, usize> = HashMap::new();
    for m in model.meshes.iter().filter(|m| !m.indices.is_empty()) {
        let key = m.tex.diffuse.filter(|&ti| ti < model.textures.len());
        let pi = match page_of.get(&key) {
            Some(&i) => i,
            None => {
                let label = match key {
                    Some(ti) => format!(
                        "texture #{ti} · {}×{}",
                        model.textures[ti].width, model.textures[ti].height
                    ),
                    None => "(no diffuse texture)".to_string(),
                };
                pages.push(UvPage {
                    label,
                    tex: key,
                    tris: Vec::new(),
                });
                page_of.insert(key, pages.len() - 1);
                pages.len() - 1
            }
        };
        for c in m.indices.chunks_exact(3) {
            // Indices come straight from the file, so some can overrun the vertex list.
            let uv = |i: u32| m.vertices.get(i as usize).map(|v| v.uv);
            if let (Some(a), Some(b), Some(cc)) = (uv(c[0]), uv(c[1]), uv(c[2])) {
                pages[pi].tris.push([a, b, cc]);
            }
        }
        if let Some(ti) = key
            && images[ti].is_none()
        {
            let t = &model.textures[ti];
            images[ti] = Some(egui::ColorImage::from_rgba_unmultiplied(
                [t.width as usize, t.height as usize],
                &t.rgba,
            ));
        }
    }
    (pages, images)
}

fn dump_wgsl(viewer: &ModelViewer) -> String {
    use super::shader_transpile;
    let Some(path) = viewer.loaded_for.as_deref() else {
        return "no model loaded".into();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return "could not read .trb".into();
    };
    let Ok(trb) = Trb::parse(&bytes) else {
        return "not a parseable .trb".into();
    };
    let Some(dir) = rfd::FileDialog::new().pick_folder() else {
        return "dump cancelled".into();
    };
    let names = trb.resource_names();
    let (mut ok, mut fail) = (0, 0);
    for i in 0..trb.resource_count() {
        let d = trb.resource_data(i).unwrap_or(&[]);
        if !d.starts_with(b"SEDBshd") {
            continue;
        }
        let Some(sh) = ff13::formats::sedbshd::main_pixel_shader(d) else {
            continue;
        };
        let label = names
            .get(i)
            .map(|n| n.rsplit(['\\', '/']).next().unwrap_or(n).to_string())
            .unwrap_or_else(|| format!("material_{i}"));
        match shader_transpile::transpile(&sh, &std::collections::BTreeSet::new()) {
            Ok(t) => {
                if std::fs::write(dir.join(format!("{label}.ps.wgsl")), &t.wgsl).is_ok() {
                    ok += 1;
                } else {
                    fail += 1;
                }
            }
            Err(e) => {
                let _ = std::fs::write(dir.join(format!("{label}.ERROR.txt")), e);
                fail += 1;
            }
        }
    }
    format!(
        "dumped {ok} WGSL file(s), {fail} failed → {}",
        dir.display()
    )
}

fn b2f(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

fn bounds(meshes: &[MeshData]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for m in meshes {
        for v in &m.vertices {
            let p = Vec3::from(v.pos);
            min = min.min(p);
            max = max.max(p);
        }
    }
    if min.is_finite() {
        (min, max)
    } else {
        (Vec3::splat(-1.0), Vec3::splat(1.0))
    }
}

#[derive(Clone, Copy)]
struct MapToggle {
    enabled: bool,
    chan: [bool; 4],
}

impl Default for MapToggle {
    fn default() -> Self {
        Self {
            enabled: true,
            chan: [true; 4],
        }
    }
}

const MAP_NAMES: [&str; 5] = ["diffuse", "normal", "specular", "opacity", "tone"];

/// Kept so the scene can be rebuilt on a light or shader change without re-reading disk.
struct ExtraModel {
    path: PathBuf,
    name: String,
    model: Model,
    visible: bool,
    /// Nonzero, so hide and remove are cheap without a rebuild.
    id: usize,
}

struct ExtraLoading {
    path: PathBuf,
    name: String,
    rx: std::sync::mpsc::Receiver<Loaded>,
}

/// Already turned into GPU meshes, so installing it does no UI-thread work.
struct Loaded {
    model_id: usize,
    model: Model,
    rig: Option<Rig>,
    meshes: Vec<GpuMesh>,
    tex_views: Vec<wgpu::TextureView>,
    skeleton_vbuf: Option<wgpu::Buffer>,
    skeleton_verts: u32,
    min: Vec3,
    max: Vec3,
    /// Scanned on the worker thread; extras skip it.
    clips: Vec<ClipRef>,
}

/// UV pages are left for the main thread to build lazily.
fn build_loaded(ctx: &BuildCtx, model: Model, model_id: usize) -> Loaded {
    let rig = model.skeleton.as_ref().map(Rig::new);
    let (skin, lines) = match &rig {
        Some(r) => r.solve(),
        None => (Vec::new(), Vec::new()),
    };
    let (meshes, tex_views) = ctx.build_meshes(
        &model,
        (!skin.is_empty()).then_some(skin.as_slice()),
        model_id,
    );
    let skeleton_verts = lines.len() as u32;
    let skeleton_vbuf = (!lines.is_empty()).then(|| ctx.build_lines(&lines));
    let (min, max) = if model.meshes.is_empty() {
        (Vec3::ZERO, Vec3::ZERO)
    } else {
        bounds(&model.meshes)
    };
    Loaded {
        model_id,
        model,
        rig,
        meshes,
        tex_views,
        skeleton_vbuf,
        skeleton_verts,
        min,
        max,
        clips: Vec::new(),
    }
}

/// A few jobs per frame, behind a progress bar.
struct Compile {
    jobs: Vec<CompileJob>,
    done: usize,
    total: usize,
}

pub struct ModelViewer {
    gpu: Option<Gpu>,
    gpu_init: Option<std::sync::mpsc::Receiver<Gpu>>,
    loaded_for: Option<PathBuf>,
    status: Option<String>,
    cam: Camera,
    wireframe: bool,
    force_two_sided: bool,
    real_shader: bool,
    light: LightRig,
    applied_light: LightRig,
    maps_toggle: [MapToggle; 5],
    uv_mode: bool,
    uv_show_texture: bool,
    uv_page: usize,
    uv_pages: Vec<UvPage>,
    uv_images: Vec<Option<egui::ColorImage>>,
    uv_handles: Vec<Option<egui::TextureHandle>>,
    /// Built lazily, when UV mode is first opened.
    uv_built: bool,
    show_skeleton: bool,
    rig: Option<Rig>,
    clips: Vec<ClipRef>,
    clip: Option<usize>,
    loading: Option<std::sync::mpsc::Receiver<Loaded>>,
    model: Option<Model>,
    real_vs: bool,
    extras: Vec<ExtraModel>,
    extra_loading: Vec<ExtraLoading>,
    next_extra_id: usize,
    reals_built: bool,
    compiling: Option<Compile>,
}

impl Default for ModelViewer {
    fn default() -> Self {
        Self {
            gpu: None,
            gpu_init: None,
            loaded_for: None,
            status: None,
            cam: Camera::default(),
            wireframe: false,
            force_two_sided: false,
            real_shader: false,
            light: LightRig::default(),
            applied_light: LightRig::default(),
            maps_toggle: [MapToggle::default(); 5],
            uv_mode: false,
            uv_show_texture: true,
            uv_page: 0,
            uv_pages: Vec::new(),
            uv_images: Vec::new(),
            uv_handles: Vec::new(),
            uv_built: false,
            show_skeleton: false,
            rig: None,
            clips: Vec::new(),
            clip: None,
            loading: None,
            model: None,
            // The game pixel and vertex shaders are coupled; `FF13_SYNTH_VS` falls back to the
            // synthesized VS for a model whose real one is broken.
            real_vs: std::env::var("FF13_SYNTH_VS").is_err(),
            extras: Vec::new(),
            extra_loading: Vec::new(),
            next_extra_id: 1,
            reals_built: false,
            compiling: None,
        }
    }
}

fn pose_controls(rig: &mut Rig, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let prev = rig.selected;
    ui.horizontal(|ui| {
        let label = rig
            .selected
            .and_then(|i| rig.joints.get(i))
            .map(|j| j.name.clone())
            .unwrap_or_else(|| "select a joint".into());
        egui::ComboBox::from_id_salt("ff13-pose-joint")
            .selected_text(label)
            .width(220.0)
            .show_ui(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (i, j) in rig.joints.iter().enumerate() {
                            ui.selectable_value(&mut rig.selected, Some(i), &j.name);
                        }
                    });
            });
        if ui.button("reset all").clicked() {
            rig.pose.iter_mut().for_each(|e| *e = Vec3::ZERO);
            changed = true;
        }
    });
    changed |= rig.selected != prev;

    if let Some(i) = rig.selected {
        let e = &mut rig.pose[i];
        let mut deg = [e.x.to_degrees(), e.y.to_degrees(), e.z.to_degrees()];
        ui.horizontal(|ui| {
            for (k, axis) in ["pitch", "yaw", "roll"].iter().enumerate() {
                changed |= ui
                    .add(egui::Slider::new(&mut deg[k], -180.0..=180.0).text(*axis))
                    .changed();
            }
            if ui.button("reset joint").clicked() {
                deg = [0.0; 3];
                changed = true;
            }
        });
        *e = Vec3::new(
            deg[0].to_radians(),
            deg[1].to_radians(),
            deg[2].to_radians(),
        );
    } else {
        ui.weak("pick a joint to rotate it; its children follow.");
    }
    changed
}

pub fn show(
    viewer: &mut ModelViewer,
    rs: &egui_wgpu::RenderState,
    path: &Path,
    adds: &[PathBuf],
    ui: &mut egui::Ui,
) {
    // Off the UI thread, so the first model click does not freeze.
    if viewer.gpu.is_none() {
        if viewer.gpu_init.is_none() {
            let rs = rs.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(Gpu::new(&rs));
            });
            viewer.gpu_init = Some(rx);
        }
        if let Some(rx) = &viewer.gpu_init
            && let Ok(gpu) = rx.try_recv()
        {
            viewer.gpu = Some(gpu);
            viewer.gpu_init = None;
        }
        if viewer.gpu.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("initializing 3D…");
            });
            ui.ctx().request_repaint();
            return;
        }
    }
    if viewer.loaded_for.as_deref() != Some(path) {
        start_load(viewer, path);
        viewer.loaded_for = Some(path.to_path_buf());
        viewer.extras.clear();
        viewer.extra_loading.clear();
        viewer.next_extra_id = 1;
    }
    for add in adds {
        let is_primary = viewer.loaded_for.as_deref() == Some(add.as_path());
        let known = viewer.extras.iter().any(|e| e.path == *add)
            || viewer.extra_loading.iter().any(|l| l.path == *add);
        if !is_primary && !known {
            start_extra_load(viewer, add);
        }
    }
    if let Some(rx) = &viewer.loading {
        match rx.try_recv() {
            Ok(loaded) => {
                finish_load(viewer, loaded);
                viewer.loading = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint(),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                viewer.loading = None;
                viewer.status = Some("load failed".into());
            }
        }
    }
    drain_extras(viewer, ui);

    ui.horizontal(|ui| {
        if viewer.loading.is_some() {
            ui.spinner();
            ui.label("loading model…");
        } else if let Some(s) = &viewer.status {
            ui.label(s);
        }
        ui.checkbox(&mut viewer.uv_mode, "UV layout");
        ui.add_enabled_ui(!viewer.uv_mode, |ui| {
            ui.checkbox(&mut viewer.real_shader, "game shader (exp.)")
                .on_hover_text(
                    "Render each material with its own transpiled game pixel + vertex shader.",
                );
            if !viewer.real_shader {
                ui.checkbox(&mut viewer.wireframe, "wireframe");
                ui.checkbox(&mut viewer.force_two_sided, "force 2-sided");
            }
            if viewer.rig.is_some() {
                ui.checkbox(&mut viewer.show_skeleton, "skeleton");
            }
            if viewer.real_shader
                && ui
                    .button("dump WGSL…")
                    .on_hover_text("Transpile every material's pixel shader to WGSL files")
                    .clicked()
            {
                viewer.status = Some(dump_wgsl(viewer));
            }
        });
    });

    if !viewer.extras.is_empty() || !viewer.extra_loading.is_empty() {
        scene_list(viewer, ui);
    }

    // Built incrementally behind a progress bar, rather than freezing the UI.
    if viewer.compiling.is_some() && !viewer.real_shader {
        viewer.compiling = None;
    }
    if viewer.real_shader && viewer.compiling.is_none() && !viewer.reals_built {
        start_compile(viewer);
    }
    if viewer.compiling.is_some() {
        compile_step(viewer, ui);
        return;
    }

    if viewer.uv_mode {
        show_uv(viewer, ui);
        return;
    }
    if viewer.real_shader {
        let total = viewer.gpu.as_ref().map(|g| g.meshes.len()).unwrap_or(0);
        let with_real = viewer
            .gpu
            .as_ref()
            .map(|g| g.meshes.iter().filter(|m| m.real.is_some()).count())
            .unwrap_or(0);
        ui.weak(format!(
            "game shader: {with_real}/{total} mesh(es) (rest fall back to the built-in shader); lighting is ours, not the game scene"
        ));
        egui::CollapsingHeader::new("lighting")
            .id_salt("real_light")
            .default_open(false)
            .show(ui, |ui| {
                let l = &mut viewer.light;
                let s = |ui: &mut egui::Ui,
                         v: &mut f32,
                         range: std::ops::RangeInclusive<f32>,
                         text: &str| {
                    ui.add(egui::Slider::new(v, range).text(text));
                };
                ui.weak("key light");
                s(
                    ui,
                    &mut l.key_azim,
                    -std::f32::consts::PI..=std::f32::consts::PI,
                    "azimuth",
                );
                s(ui, &mut l.key_elev, -0.2..=1.5, "elevation");
                s(ui, &mut l.key_intensity, 0.0..=2.0, "intensity");
                s(ui, &mut l.key_warmth, -1.0..=1.0, "warmth");
                ui.weak("ambient");
                s(ui, &mut l.ambient, 0.0..=0.5, "level");
                s(ui, &mut l.ambient_warmth, -1.0..=1.0, "warmth");
                ui.weak("surface");
                s(ui, &mut l.gloss, 0.0..=2.0, "gloss (reflections)");
                ui.weak("output");
                s(ui, &mut l.gamma_exp, 0.45..=1.3, "gamma (lower = brighter)");
                if ui.button("reset to default").clicked() {
                    *l = LightRig::default();
                }
            });
        if viewer.light != viewer.applied_light {
            // Only gloss feeds the reflection cube; the rest are uniform-only updates.
            let rebuild_reflection = viewer.light.gloss != viewer.applied_light.gloss;
            if let Some(gpu) = viewer.gpu.as_ref() {
                let (anim, frame) = anim_state(viewer);
                gpu.relight(&viewer.light, None, rebuild_reflection, anim, frame);
            }
            viewer.applied_light = viewer.light;
        }
        // Eyes want the per-frame dynamic scene cube, which is in no file, and baked probes are
        // set geometry that looks wrong on a face, hence the stylized stand-in.
    }

    if !viewer.real_shader {
        egui::CollapsingHeader::new("texture & channel toggles")
            .id_salt("tex_toggles")
            .default_open(false)
            .show(ui, |ui| {
                ui.weak("name = map on/off, then per-channel R G B A");
                for (i, name) in MAP_NAMES.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let t = &mut viewer.maps_toggle[i];
                        ui.toggle_value(&mut t.enabled, *name);
                        let on = t.enabled;
                        ui.add_enabled_ui(on, |ui| {
                            for (c, lbl) in ["R", "G", "B", "A"].iter().enumerate() {
                                ui.toggle_value(&mut t.chan[c], *lbl);
                            }
                        });
                    });
                }
            });
    }
    ui.weak("drag: orbit · right-drag: pan · scroll: zoom");

    let mut posed_changed = false;
    if !viewer.clips.is_empty() {
        let mut pick = viewer.clip;
        let label = match pick {
            None => "bind pose".to_string(),
            Some(i) => viewer
                .clips
                .get(i)
                .map(|c| c.label.clone())
                .unwrap_or_default(),
        };
        ui.horizontal(|ui| {
            ui.label("animation:");
            egui::ComboBox::from_id_salt("ff13-clip")
                .selected_text(label)
                .width(260.0)
                .show_ui(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            if ui
                                .add(egui::SelectableLabel::new(pick.is_none(), "bind pose"))
                                .clicked()
                            {
                                pick = None;
                            }
                            for (i, c) in viewer.clips.iter().enumerate() {
                                let text = if c.supported {
                                    c.label.clone()
                                } else {
                                    format!("{} (unsupported)", c.label)
                                };
                                let item = egui::SelectableLabel::new(pick == Some(i), text);
                                if ui.add_enabled(c.supported, item).clicked() {
                                    pick = Some(i);
                                }
                            }
                        });
                });
            let playable = viewer.clips.iter().filter(|c| c.supported).count();
            ui.weak(format!("{playable}/{} clip(s)", viewer.clips.len()));
        });
        if pick != viewer.clip {
            viewer.clip = pick;
            if let Some(rig) = viewer.rig.as_mut() {
                match pick {
                    Some(i) => load_clip(rig, &viewer.clips[i]),
                    None => rig.anim = None,
                }
                posed_changed = true;
            }
        }
    } else if viewer.rig.is_some() {
        ui.weak("no animation packs found for this model");
    }

    if let Some(rig) = viewer.rig.as_mut() {
        if let Some((fps, frames)) = rig.anim.as_ref().map(|m| (m.fps, m.frames)) {
            ui.horizontal(|ui| {
                ui.checkbox(&mut rig.playing, "▶ play");
                if ui
                    .add(egui::Slider::new(&mut rig.frame, 0.0..=frames).text("frame"))
                    .changed()
                {
                    posed_changed = true;
                }
                if ui.checkbox(&mut rig.root_motion, "root motion").changed() {
                    posed_changed = true;
                }
                ui.weak(format!("{fps:.0} fps · {frames:.0} frames"));
            });
            if rig.playing {
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                rig.frame = (rig.frame + dt * fps).rem_euclid(frames.max(1.0));
                posed_changed = true;
                ui.ctx().request_repaint();
            }
        }
        egui::CollapsingHeader::new("pose (FK)")
            .default_open(false)
            .show(ui, |ui| {
                posed_changed |= pose_controls(rig, ui);
            });
    }

    let size = ui.available_size();
    if size.x < 16.0 || size.y < 16.0 {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    let d = resp.drag_delta();
    if resp.dragged_by(egui::PointerButton::Primary) {
        viewer.cam.yaw -= d.x * 0.01;
        viewer.cam.pitch = (viewer.cam.pitch - d.y * 0.01).clamp(-1.55, 1.55);
    }
    if resp.dragged_by(egui::PointerButton::Secondary) {
        viewer.cam.pan(d.x, d.y);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let cam = &mut viewer.cam;
            cam.distance = (cam.distance * (1.0 - scroll * 0.0015))
                .clamp(cam.radius * 0.05, cam.radius * 60.0);
        }
    }

    let ppp = ui.ctx().pixels_per_point();
    let px = ((rect.width() * ppp).round() as u32).clamp(1, 4096);
    let py = ((rect.height() * ppp).round() as u32).clamp(1, 4096);

    let aspect = px as f32 / py as f32;
    let light = Vec3::new(-0.4, -1.0, -0.55).normalize();
    let eye = viewer.cam.eye();
    let tg = &viewer.maps_toggle;
    let mask = |i: usize| tg[i].chan.map(b2f);
    let uniforms = Uniforms {
        view_proj: viewer.cam.view_proj(aspect),
        light_dir: [light.x, light.y, light.z, 0.0],
        eye: [eye.x, eye.y, eye.z, 0.0],
        uv_xform: [1.0, -1.0, 0.0, 1.0],
        flags: [
            0.0,
            if viewer.wireframe { 1.0 } else { 0.0 },
            if viewer.force_two_sided { 1.0 } else { 0.0 },
            0.0,
        ],
        diffuse_mask: mask(0),
        normal_mask: mask(1),
        specular_mask: mask(2),
        opacity_mask: mask(3),
        tone_mask: mask(4),
        map_on: [
            b2f(tg[0].enabled),
            b2f(tg[1].enabled),
            b2f(tg[2].enabled),
            b2f(tg[3].enabled),
        ],
        map_on2: [b2f(tg[4].enabled), 0.0, 0.0, 0.0],
    };

    if posed_changed && let (Some(rig), Some(gpu)) = (viewer.rig.as_ref(), viewer.gpu.as_mut()) {
        let (skin, lines) = rig.solve();
        gpu.repose(&skin, &lines);
        if rig.anim.as_ref().is_some_and(|m| gpu.has_material_anim(m)) {
            gpu.write_material_consts(&viewer.light, rig.anim.as_ref(), rig.frame);
        }
    }

    let real = viewer.real_shader;
    let show_skeleton = viewer.show_skeleton;
    let view = viewer.cam.matrices(aspect).0;
    let gpu = viewer.gpu.as_mut().unwrap();
    gpu.ensure_target((px, py));
    gpu.render(&uniforms, view, real, show_skeleton);

    if let Some(target) = &gpu.target {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter()
            .image(target.tex_id, rect, uv, egui::Color32::WHITE);
    }
}

fn show_uv(viewer: &mut ModelViewer, ui: &mut egui::Ui) {
    if !viewer.uv_built {
        viewer.uv_built = true;
        if let Some((pages, images)) = viewer.model.as_ref().map(build_uv) {
            viewer.uv_handles = images.iter().map(|_| None).collect();
            viewer.uv_pages = pages;
            viewer.uv_images = images;
            viewer.uv_page = 0;
        }
    }
    if viewer.uv_pages.is_empty() {
        ui.weak("no UV data in this model");
        return;
    }
    viewer.uv_page = viewer.uv_page.min(viewer.uv_pages.len() - 1);

    ui.horizontal(|ui| {
        let current = viewer.uv_pages[viewer.uv_page].label.clone();
        egui::ComboBox::from_id_salt("uv_page")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for i in 0..viewer.uv_pages.len() {
                    let label = viewer.uv_pages[i].label.clone();
                    ui.selectable_value(&mut viewer.uv_page, i, label);
                }
            });
        ui.checkbox(&mut viewer.uv_show_texture, "texture backdrop");
    });
    let tri_count = viewer.uv_pages[viewer.uv_page].tris.len();
    ui.weak(format!("{tri_count} triangle(s)"));

    let avail = ui.available_size();
    let side = avail.x.min(avail.y).max(16.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::from_gray(18));

    let tex = viewer.uv_pages[viewer.uv_page].tex;
    if viewer.uv_show_texture
        && let Some(id) = tex.and_then(|ti| uv_texture_id(viewer, ti, ui.ctx()))
    {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        p.image(id, rect, uv, egui::Color32::WHITE);
    }
    p.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)),
    );

    let to_screen = |uv: [f32; 2]| {
        egui::pos2(
            rect.left() + uv[0] * side,
            rect.top() + (1.0 - uv[1]) * side,
        )
    };
    let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 150, 90));
    let page = &viewer.uv_pages[viewer.uv_page];
    let mut shapes = Vec::with_capacity(page.tris.len() * 3);
    for tri in &page.tris {
        let a = to_screen(tri[0]);
        let b = to_screen(tri[1]);
        let c = to_screen(tri[2]);
        shapes.push(egui::Shape::line_segment([a, b], stroke));
        shapes.push(egui::Shape::line_segment([b, c], stroke));
        shapes.push(egui::Shape::line_segment([c, a], stroke));
    }
    p.extend(shapes);
}

fn uv_texture_id(
    viewer: &mut ModelViewer,
    ti: usize,
    ctx: &egui::Context,
) -> Option<egui::TextureId> {
    if ti >= viewer.uv_handles.len() {
        return None;
    }
    if viewer.uv_handles[ti].is_none() {
        let img = viewer.uv_images.get_mut(ti)?.take()?;
        let handle = ctx.load_texture(format!("ff13uv_{ti}"), img, egui::TextureOptions::LINEAR);
        viewer.uv_handles[ti] = Some(handle);
    }
    Some(viewer.uv_handles[ti].as_ref()?.id())
}

/// Also reads any sibling package holding textures it references, since some models ship a
/// zero-byte `.imgb` and draw everything from elsewhere.
pub(crate) fn read_model(path: &Path) -> ff13::formats::model::Model {
    let trb = std::fs::read(path).unwrap_or_default();
    let imgb = std::fs::read(path.with_extension("imgb")).unwrap_or_default();
    // The reference can chain through several TRBs, each with an empty `.imgb`.
    let mut packages: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut queue = ff13::formats::model::texture_packages(&trb);
    while let Some(id) = queue.pop() {
        if seen.contains(&id) || seen.len() > 8 {
            continue;
        }
        seen.push(id.clone());
        let Some(root) = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        else {
            break;
        };
        let p = root.join(&id).join("bin").join(format!("{id}.win32.trb"));
        let Ok(pkg) = std::fs::read(&p) else { continue };
        queue.extend(ff13::formats::model::texture_packages(&pkg));
        packages.push((
            pkg,
            std::fs::read(p.with_extension("imgb")).unwrap_or_default(),
        ));
    }
    ff13::formats::model::parse_with_packages(&trb, &imgb, &packages)
}

fn start_load(viewer: &mut ModelViewer, path: &Path) {
    // All of it on the worker, so the main thread only installs the finished package.
    let Some(ctx) = viewer.gpu.as_ref().map(Gpu::build_ctx) else {
        return;
    };
    let path = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let timed = std::env::var("FF13_TIME").is_ok();
        let t = std::time::Instant::now();
        let read_ms = t.elapsed().as_secs_f32() * 1000.0;
        let t = std::time::Instant::now();
        let model = read_model(&path);
        let parse_ms = t.elapsed().as_secs_f32() * 1000.0;
        let ntex = model.textures.len();
        let t = std::time::Instant::now();
        let mut loaded = build_loaded(&ctx, model, 0);
        loaded.clips = list_clips(&path);
        if timed {
            eprintln!(
                "[ff13 load] read={read_ms:.0}ms parse={parse_ms:.0}ms build_gpu={:.0}ms ({ntex} tex)",
                t.elapsed().as_secs_f32() * 1000.0,
            );
        }
        let _ = tx.send(loaded);
    });
    viewer.loading = Some(rx);
    viewer.status = None;
    viewer.uv_pages.clear();
    viewer.uv_images.clear();
    viewer.uv_handles.clear();
    viewer.rig = None;
    viewer.model = None;
    viewer.clips.clear();
    viewer.clip = None;
    viewer.reals_built = false;
    viewer.compiling = None;
    if let Some(gpu) = viewer.gpu.as_mut() {
        gpu.meshes.clear();
        gpu.reals.clear();
        gpu.skeleton_vbuf = None;
        gpu.skeleton_verts = 0;
        gpu.hidden.clear();
    }
}

fn finish_load(viewer: &mut ModelViewer, loaded: Loaded) {
    if loaded.model.meshes.is_empty() {
        if let Some(gpu) = viewer.gpu.as_mut() {
            gpu.install_primary(Vec::new(), Vec::new(), None, 0);
        }
        viewer.uv_pages.clear();
        viewer.rig = None;
        viewer.model = None;
        viewer.status = Some("no 3D geometry in this .trb (it may be textures only)".into());
        return;
    }
    viewer.cam = Camera::framing(loaded.min, loaded.max);
    let verts: usize = loaded.model.meshes.iter().map(|m| m.vertices.len()).sum();
    let tris: usize = loaded
        .model
        .meshes
        .iter()
        .map(|m| m.indices.len())
        .sum::<usize>()
        / 3;
    viewer.status = Some(format!(
        "{} mesh(es) · {verts} verts · {tris} tris · {} texture(s)",
        loaded.model.meshes.len(),
        loaded.model.textures.len(),
    ));
    if let Some(gpu) = viewer.gpu.as_mut() {
        gpu.install_primary(
            loaded.meshes,
            loaded.tex_views,
            loaded.skeleton_vbuf,
            loaded.skeleton_verts,
        );
    }
    viewer.uv_pages = Vec::new();
    viewer.uv_images = Vec::new();
    viewer.uv_handles = Vec::new();
    viewer.uv_page = 0;
    viewer.uv_built = false;
    viewer.rig = loaded.rig;
    viewer.clips = loaded.clips;
    viewer.clip = None;
    viewer.model = Some(loaded.model);
    viewer.reals_built = false;
    viewer.compiling = None;
}

/// Resets any current pipelines; compilation itself runs in `compile_step`.
fn start_compile(viewer: &mut ModelViewer) {
    let mut models: Vec<(usize, &Model)> = Vec::new();
    if let Some(m) = &viewer.model {
        models.push((0, m));
    }
    // Even hidden extras, so unhiding one later does not reveal an uncompiled model.
    for e in &viewer.extras {
        models.push((e.id, &e.model));
    }
    let jobs = match viewer.gpu.as_ref() {
        Some(gpu) => gpu.plan_compile(&models),
        None => Vec::new(),
    };
    drop(models);
    if let Some(gpu) = viewer.gpu.as_mut() {
        gpu.reset_reals();
    }
    if jobs.is_empty() {
        viewer.reals_built = true;
        viewer.compiling = None;
    } else {
        let total = jobs.len();
        viewer.compiling = Some(Compile {
            jobs,
            done: 0,
            total,
        });
    }
}

/// Returns once the whole scene is compiled, and the normal render resumes next frame.
fn compile_step(viewer: &mut ModelViewer, ui: &mut egui::Ui) {
    const BUDGET: usize = 2;
    let real_vs = viewer.real_vs;
    let light = viewer.light;
    if let (Some(c), Some(gpu)) = (viewer.compiling.as_mut(), viewer.gpu.as_mut()) {
        let mut n = 0;
        while c.done < c.total && n < BUDGET {
            gpu.compile_job(&c.jobs[c.done], real_vs, &light);
            c.done += 1;
            n += 1;
        }
    }
    let (done, total) = viewer
        .compiling
        .as_ref()
        .map_or((0, 0), |c| (c.done, c.total));
    ui.ctx().request_repaint();
    if done >= total {
        if let Some(gpu) = viewer.gpu.as_ref() {
            let (anim, frame) = anim_state(viewer);
            gpu.relight(&light, None, true, anim, frame);
        }
        viewer.compiling = None;
        viewer.reals_built = true;
        return;
    }
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.heading("Compiling game shaders…");
        ui.add_space(10.0);
        ui.add(
            egui::ProgressBar::new(done as f32 / total as f32)
                .desired_width(320.0)
                .text(format!("{done} / {total}")),
        );
    });
}

fn reframe(viewer: &mut ModelViewer) {
    let mut acc: Option<(Vec3, Vec3)> = viewer.model.as_ref().map(|m| bounds(&m.meshes));
    for e in viewer.extras.iter().filter(|e| e.visible) {
        let (a, b) = bounds(&e.model.meshes);
        acc = Some(match acc {
            Some((mn, mx)) => (mn.min(a), mx.max(b)),
            None => (a, b),
        });
    }
    if let Some((mn, mx)) = acc {
        viewer.cam = Camera::framing(mn, mx);
    }
}

fn start_extra_load(viewer: &mut ModelViewer, path: &Path) {
    let Some(ctx) = viewer.gpu.as_ref().map(Gpu::build_ctx) else {
        return;
    };
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = viewer.next_extra_id;
    viewer.next_extra_id += 1;
    let load_path = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let model = read_model(&load_path);
        let _ = tx.send(build_loaded(&ctx, model, id));
    });
    viewer.extra_loading.push(ExtraLoading {
        path: path.to_path_buf(),
        name,
        rx,
    });
}

fn drain_extras(viewer: &mut ModelViewer, ui: &mut egui::Ui) {
    let mut i = 0;
    let mut added = false;
    while i < viewer.extra_loading.len() {
        match viewer.extra_loading[i].rx.try_recv() {
            Ok(loaded) => {
                let el = viewer.extra_loading.remove(i);
                if !loaded.model.meshes.is_empty() {
                    // No scene rebuild, but game-shader mode still needs the new model compiled.
                    if let Some(gpu) = viewer.gpu.as_mut() {
                        gpu.install_extra(loaded.model_id, loaded.meshes, loaded.tex_views);
                    }
                    if viewer.real_shader {
                        viewer.reals_built = false;
                    }
                    viewer.extras.push(ExtraModel {
                        path: el.path,
                        name: el.name,
                        model: loaded.model,
                        visible: true,
                        id: loaded.model_id,
                    });
                    added = true;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint();
                i += 1;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                viewer.extra_loading.remove(i);
            }
        }
    }
    if added {
        reframe(viewer);
    }
}

fn scene_list(viewer: &mut ModelViewer, ui: &mut egui::Ui) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("scene · {} model(s)", 1 + viewer.extras.len()));
        ui.weak("shift-click a .trb to add");
    });
    let pname = viewer
        .loaded_for
        .as_deref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".into());
    ui.horizontal(|ui| {
        ui.label("•");
        ui.label(pname);
        ui.weak("(active · posable)");
    });
    let mut toggle = None;
    let mut remove = None;
    for (i, e) in viewer.extras.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut e.visible, "")
                .on_hover_text("show / hide")
                .changed()
            {
                toggle = Some((e.id, !e.visible));
            }
            ui.label(&e.name);
            if ui
                .small_button("×")
                .on_hover_text("remove from scene")
                .clicked()
            {
                remove = Some((i, e.id));
            }
        });
    }
    for l in &viewer.extra_loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak(format!("loading {}…", l.name));
        });
    }
    // A render-pass skip or a mesh drop, never a scene recompile.
    if let Some((idx, id)) = remove {
        viewer.extras.remove(idx);
        if let Some(gpu) = viewer.gpu.as_mut() {
            gpu.remove_model(id);
        }
        reframe(viewer);
    } else if let Some((id, hidden)) = toggle
        && let Some(gpu) = viewer.gpu.as_mut()
    {
        gpu.set_model_hidden(id, hidden);
    }
}

/// For layering material curves onto shader constants.
fn anim_state(viewer: &ModelViewer) -> (Option<&ff13::formats::mot::Motion>, f32) {
    match viewer.rig.as_ref() {
        Some(r) => (r.anim.as_ref(), r.frame),
        None => (None, 0.0),
    }
}
