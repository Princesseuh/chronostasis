use super::game_shader::*;
use super::*;
use crate::modder::shader_transpile;
use eframe::egui_wgpu::wgpu;
use eframe::egui_wgpu::wgpu::util::DeviceExt;
use ff13::formats::skl;
use glam::{Mat4, Vec3};

// cargo only sets `CARGO_TARGET_TMPDIR` for integration tests, not unit tests.
fn out_path(name: &str) -> std::path::PathBuf {
    option_env!("CARGO_TARGET_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(name)
}

/// Green is the aux cutout channel.
fn green_coverage(t: &TexData) -> f32 {
    let total = (t.rgba.len() / 4).max(1);
    let n = t.rgba.chunks_exact(4).filter(|c| c[1] < 128).count();
    n as f32 / total as f32
}

fn alpha_coverage(t: &TexData) -> f32 {
    let total = (t.rgba.len() / 4).max(1);
    let n = t.rgba.chunks_exact(4).filter(|c| c[3] < 200).count();
    n as f32 / total as f32
}

/// Characters cut hair two different ways, and both have failed before: no mesh may end up
/// fully erased, and each character must end up with at least one partly-cut mesh.
#[test]
fn hair_cutout_is_sane() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let root = format!("{dir}/chr/pc");
    // c004's opaque diffuse is flagged 0x03, and its body must NOT be cut.
    for ch in ["c001", "c003", "c004", "c202", "c203", "c205", "c206"] {
        let p = format!("{root}/{ch}/bin/{ch}.win32.trb");
        let Ok(trb_bytes) = std::fs::read(&p) else {
            continue;
        };
        let imgb =
            std::fs::read(std::path::Path::new(&p).with_extension("imgb")).unwrap_or_default();
        let model = ff13::formats::model::parse(&trb_bytes, &imgb);
        let mut any_cut = false;
        for m in &model.meshes {
            let cov = if let Some(o) = m.tex.opacity {
                green_coverage(&model.textures[o])
            } else if let Some(d) = m.tex.diffuse {
                let a = alpha_coverage(&model.textures[d]);
                if a > 0.02 { a } else { 0.0 }
            } else {
                0.0
            };
            if cov > 0.001 {
                any_cut = true;
                assert!(cov < 0.97, "{ch}: a mesh is ~fully erased (mask {cov:.2})");
            }
        }
        assert!(
            any_cut,
            "{ch}: no mesh got a cutout; hair would render solid"
        );
    }
}

/// The overlay only reads right if the rig shares the mesh's coordinate space, so a rig outside
/// the mesh AABB means the bind pose cannot be identity either.
#[test]
fn skeleton_aligns_with_mesh() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    for ch in ["c001", "c201"] {
        let p = format!("{dir}/chr/pc/{ch}/bin/{ch}.win32.trb");
        let Ok(trb) = std::fs::read(&p) else { continue };
        let model = ff13::formats::model::parse(&trb, &[]);
        let (mmin, mmax) = bounds(&model.meshes);
        let Some(skel) = model.skeleton.as_ref() else {
            panic!("{ch} should carry a skeleton")
        };
        let (_, lines) = Rig::new(skel).solve();
        let mut smin = Vec3::splat(f32::INFINITY);
        let mut smax = Vec3::splat(f32::NEG_INFINITY);
        for pt in &lines {
            let v = Vec3::from(pt.pos);
            smin = smin.min(v);
            smax = smax.max(v);
        }
        // Bones live under the skin, so a little slack is expected.
        let pad = (mmax - mmin) * 0.15 + Vec3::splat(0.05);
        assert!(
            smin.cmpge(mmin - pad).all() && smax.cmple(mmax + pad).all(),
            "{ch}: skeleton extent {smin:?}..{smax:?} escapes mesh {mmin:?}..{mmax:?}"
        );
    }
}

/// A headless oracle, so animation poses can be eyeballed without the GUI.
fn render_skeleton_png(rig: &Rig, path: &std::path::Path) {
    render_world_png(&rig.joints, &rig.posed_world(), path);
}

fn render_world_png(joints: &[skl::Joint], world: &[Mat4], path: &std::path::Path) {
    let pts: Vec<Vec3> = (0..world.len())
        .map(|i| world[i].col(3).truncate())
        .collect();
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for p in &pts {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let center = (mn + mx) * 0.5;
    let radius = ((mx - mn).length() * 0.5).max(0.1);
    let eye = center + Vec3::new(radius * 1.2, radius * 0.2, radius * 2.6);
    let view = Mat4::look_at_rh(eye, center, Vec3::Y);
    let (w, h) = (640u32, 880u32);
    let proj = Mat4::perspective_rh(45f32.to_radians(), w as f32 / h as f32, 0.01, radius * 8.0);
    let vp = proj * view;
    let project = |p: Vec3| -> Option<(i32, i32)> {
        let c = vp * p.extend(1.0);
        if c.w <= 0.0 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        Some((
            ((ndc.x * 0.5 + 0.5) * w as f32) as i32,
            ((1.0 - (ndc.y * 0.5 + 0.5)) * h as f32) as i32,
        ))
    };
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([18, 18, 24, 255]));
    let mut line = |a: (i32, i32), b: (i32, i32), col: [u8; 3]| {
        let (mut x0, mut y0) = a;
        let (x1, y1) = b;
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            if (0..w as i32).contains(&x0) && (0..h as i32).contains(&y0) {
                img.put_pixel(
                    x0 as u32,
                    y0 as u32,
                    image::Rgba([col[0], col[1], col[2], 255]),
                );
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    };
    for (i, j) in joints.iter().enumerate() {
        if j.parent >= 0
            && let (Some(a), Some(b)) = (project(pts[i]), project(pts[j.parent as usize]))
        {
            line(a, b, [120, 230, 140]);
        }
    }
    for p in &pts {
        if let Some((x, y)) = project(*p) {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
                let (px, py) = (x + dx, y + dy);
                if (0..w as i32).contains(&px) && (0..h as i32).contains(&py) {
                    img.put_pixel(px as u32, py as u32, image::Rgba([255, 210, 80, 255]));
                }
            }
        }
    }
    img.save(path).unwrap();
    eprintln!("wrote {}", path.display());
}

// c201 rather than c001, whose only clip is in the unsupported old format.
#[test]
fn rig_anim_oracle() {
    let Ok(g) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let tp = format!("{g}/chr/pc/c201/bin/c201.win32.trb");
    let Ok(tb) = std::fs::read(&tp) else { return };
    let skel = ff13::formats::trb::Trb::parse(&tb)
        .unwrap()
        .skeleton()
        .unwrap();
    let mut rig = Rig::new(&skel);
    let clips = list_clips(std::path::Path::new(&tp));
    let clip = clips
        .iter()
        .find(|c| c.supported)
        .expect("a supported clip");
    load_clip(&mut rig, clip);
    assert!(rig.anim.is_some(), "clip load failed");
    rig.frame = 30.0;
    render_skeleton_png(&rig, &out_path("rig_anim_f030.png"));
}

#[test]
fn skeleton_oracle_bind() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let p = format!("{dir}/chr/pc/c001/bin/c001.win32.trb");
    let Ok(tb) = std::fs::read(&p) else { return };
    let skel = ff13::formats::trb::Trb::parse(&tb)
        .unwrap()
        .skeleton()
        .unwrap();
    let rig = Rig::new(&skel);
    render_skeleton_png(&rig, &out_path("pose_bind.png"));
}

/// The real body silhouette rather than just the skeleton, which is what distinguishes real
/// motion from a decode error.
#[test]
fn c201_mesh_oracle() {
    let Ok(gd) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let base = format!("{gd}/chr/pc/c201/bin/c201.win32");
    let (Ok(tb), Ok(ib)) = (
        std::fs::read(format!("{base}.trb")),
        std::fs::read(format!("{base}.imgb")),
    ) else {
        return;
    };
    let model = ff13::formats::model::parse(&tb, &ib);
    let Some(skel) = &model.skeleton else { return };
    let mp = format!("{gd}/mot/pc/sk_c201_lt/s1.white.win32.bin");
    let Ok(b) = std::fs::read(&mp) else { return };
    let motion = ff13::formats::mot::Motion::from_wpd_named(&b, "s1attack5").unwrap();
    eprintln!(
        "c201 s1attack5: fps={} frames={} bones={}",
        motion.fps,
        motion.frames,
        motion.animated_bones()
    );
    let inv_bind: Vec<Mat4> = skel
        .inverse_bind()
        .iter()
        .map(Mat4::from_cols_array_2d)
        .collect();
    let n = skel.joints.len();
    for frame in [
        None,
        Some(0.0f32),
        Some(15.0),
        Some(30.0),
        Some(45.0),
        Some(60.0),
    ] {
        let local: Vec<Mat4> = (0..n)
            .map(|i| match frame {
                None => Mat4::from_cols_array_2d(&skel.joints[i].local_matrix()),
                Some(fr) => Mat4::from_cols_array_2d(&motion.local(
                    i,
                    fr,
                    skel.joints[i].local_matrix(),
                    false,
                )),
            })
            .collect();
        let mut world = vec![Mat4::IDENTITY; n];
        for i in 0..n {
            let mut m = local[i];
            let mut p = skel.joints[i].parent;
            let mut g = 0;
            while p >= 0 && (p as usize) < n && g < n {
                m = local[p as usize] * m;
                p = skel.joints[p as usize].parent;
                g += 1;
            }
            world[i] = m;
        }
        let skin: Vec<Mat4> = world
            .iter()
            .zip(&inv_bind)
            .map(|(w, ib)| *w * *ib)
            .collect();
        let mut pts = Vec::new();
        for mesh in &model.meshes {
            for v in &mesh.vertices {
                pts.push(skinned_pos(v, &mesh.palette, &skin));
            }
        }
        let tag = frame
            .map(|f| format!("f{:03}", f as u32))
            .unwrap_or_else(|| "bind".into());
        for (axis, view) in [(0, "front"), (1, "side")] {
            render_points_png(&pts, axis, &out_path(&format!("c201_{tag}_{view}.png")));
        }
    }
}

fn render_points_png(pts: &[Vec3], axis: u8, path: &std::path::Path) {
    let (mut mn, mut mx) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for p in pts {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let center = (mn + mx) * 0.5;
    let radius = ((mx - mn).length() * 0.5).max(0.1);
    let off = if axis == 0 {
        Vec3::new(0.0, 0.0, radius * 2.6)
    } else {
        Vec3::new(radius * 2.6, 0.0, 0.0)
    };
    let eye = center + off;
    let view = Mat4::look_at_rh(eye, center, Vec3::Y);
    let (w, h) = (520u32, 760u32);
    let proj = Mat4::perspective_rh(45f32.to_radians(), w as f32 / h as f32, 0.01, radius * 8.0);
    let vp = proj * view;
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([12, 12, 18, 255]));
    for p in pts {
        let c = vp * p.extend(1.0);
        if c.w <= 0.0 {
            continue;
        }
        let ndc = c.truncate() / c.w;
        let (x, y) = (
            ((ndc.x * 0.5 + 0.5) * w as f32) as i32,
            ((1.0 - (ndc.y * 0.5 + 0.5)) * h as f32) as i32,
        );
        if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
            img.put_pixel(x as u32, y as u32, image::Rgba([130, 235, 150, 255]));
        }
    }
    img.save(path).unwrap();
    eprintln!("wrote {}", path.display());
}

/// A CPU mirror of the vertex shader's skinning, so the posing math can be checked without a GPU.
fn skinned_pos(v: &Vertex, palette: &[u32], skin: &[Mat4]) -> Vec3 {
    let wsum: f32 = v.weights.iter().sum();
    if wsum <= 0.001 {
        return Vec3::from(v.pos);
    }
    let mut m = Mat4::ZERO;
    for k in 0..4 {
        let joint = palette
            .get(v.joints[k] as usize)
            .copied()
            .unwrap_or(u32::MAX);
        let bone = skin.get(joint as usize).copied().unwrap_or(Mat4::IDENTITY);
        m += bone * v.weights[k];
    }
    m.transform_point3(Vec3::from(v.pos))
}

/// Exercises the palette-to-joint-to-skin indirection the shader relies on: at the bind pose the
/// skinned mesh must equal the raw one, and rotating the root must move all of it.
#[test]
fn posing_deforms_mesh() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let p = format!("{dir}/chr/pc/c001/bin/c001.win32.trb");
    let Ok(trb) = std::fs::read(&p) else { return };
    let model = ff13::formats::model::parse(&trb, &[]);
    let skel = model.skeleton.as_ref().expect("c001 has a skeleton");
    let mut rig = Rig::new(skel);

    let aabb = |skin: &[Mat4]| {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for m in &model.meshes {
            for v in &m.vertices {
                let p = skinned_pos(v, &m.palette, skin);
                mn = mn.min(p);
                mx = mx.max(p);
            }
        }
        (mn, mx)
    };

    let (skin0, _) = rig.solve();
    let (bmin, bmax) = aabb(&skin0);
    let (mmin, mmax) = bounds(&model.meshes);
    assert!(
        (bmin - mmin).length() < 1e-3 && (bmax - mmax).length() < 1e-3,
        "bind-pose skinning is not identity (mesh shifted)"
    );

    let root = skel.joints.iter().position(|j| j.parent < 0).unwrap();
    rig.pose[root] = Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0);
    let (skin1, _) = rig.solve();
    let (pmin, pmax) = aabb(&skin1);
    assert!(
        (pmin - bmin).length() + (pmax - bmax).length() > 0.1,
        "posing the root did not deform the skinned mesh"
    );
}

/// Shaders and pipelines are only validated at pipeline-creation, never at build, so this
/// builds the real thing on a headless device.
#[test]
fn pipeline_builds() {
    let instance = wgpu::Instance::default();
    let Some(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping pipeline validation");
        return;
    };
    let (device, _queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).expect("device");
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let uni = uniform_bind_group_layout(&device);
    let tex = texture_bind_group_layout(&device);
    let _pipeline =
        build_model_pipeline(&device, &uni, &tex, &shader, wgpu::PolygonMode::Fill, None);
    let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test-line"),
        source: wgpu::ShaderSource::Wgsl(LINE_SHADER.into()),
    });
    let _line = build_line_pipeline(&device, &uni, &line_shader);
    let err = block_on(device.pop_error_scope());
    assert!(err.is_none(), "pipeline validation failed: {err:?}");
}

/// Every transpiled material shader must build a valid render pipeline.
#[test]
fn real_material_pipelines_build() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    // `FF13_AUDIT_DIR` widens this to every material under a tree.
    let trbs: Vec<std::path::PathBuf> = if let Ok(root) = std::env::var("FF13_AUDIT_DIR") {
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().map(|x| x == "trb").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        walk(std::path::Path::new(&root), &mut v);
        v
    } else {
        vec![std::path::PathBuf::from(format!(
            "{dir}/chr/pc/c001/bin/c001.win32.trb"
        ))]
    };

    let instance = wgpu::Instance::default();
    let Some(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let (device, _q) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();

    let (mut built, mut bad) = (0u32, 0u32);
    for p in &trbs {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let Ok(trb) = Trb::parse(&bytes) else {
            continue;
        };
        for i in 0..trb.resource_count() {
            let d = trb.resource_data(i).unwrap_or(&[]);
            if !d.starts_with(b"SEDBshd") {
                continue;
            }
            let Some(sh) = ff13::formats::sedbshd::main_pixel_shader(d) else {
                continue;
            };
            let Ok(t) =
                shader_transpile::full_module(&sh, &std::collections::BTreeSet::new(), None)
            else {
                continue;
            };
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("real"),
                source: wgpu::ShaderSource::Wgsl(t.wgsl.clone().into()),
            });
            let (g0, g1) = real_bind_group_layouts(&device, &t, false);
            let bones_l = real_bones_layout(&device);
            let _pipe =
                build_real_pipeline(&device, &g0, &g1, &bones_l, &module, &module, MSAA, true);
            if let Some(err) = block_on(device.pop_error_scope()) {
                bad += 1;
                if bad <= 5 {
                    eprintln!("INVALID {}#{i}: {err:?}", p.display());
                }
            } else {
                built += 1;
            }
        }
    }
    eprintln!("built {built} OK, {bad} invalid");
    assert!(bad == 0, "{bad} materials produced invalid WGSL");
    assert!(built > 0);
}

/// The real transpiled vertex shader, not the synthesized fallback.
#[test]
fn real_vs_modules_build() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let trbs: Vec<std::path::PathBuf> = if let Ok(root) = std::env::var("FF13_AUDIT_DIR") {
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().map(|x| x == "trb").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        walk(std::path::Path::new(&root), &mut v);
        v
    } else {
        vec![std::path::PathBuf::from(format!(
            "{dir}/chr/pc/c001/bin/c001.win32.trb"
        ))]
    };
    let instance = wgpu::Instance::default();
    let Some(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let (device, _q) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();
    let empty = std::collections::BTreeSet::new();
    let (mut ok, mut nofallback, mut bad) = (0u32, 0u32, 0u32);
    for p in &trbs {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let Ok(trb) = Trb::parse(&bytes) else {
            continue;
        };
        for i in 0..trb.resource_count() {
            let d = trb.resource_data(i).unwrap_or(&[]);
            if !d.starts_with(b"SEDBshd") {
                continue;
            }
            let (Some(vs), Some(ps)) = (
                ff13::formats::sedbshd::main_vertex_shader(d),
                ff13::formats::sedbshd::main_pixel_shader(d),
            ) else {
                continue;
            };
            let Ok(t) = shader_transpile::full_module(&ps, &empty, Some(&vs)) else {
                continue;
            };
            if t.vs_wgsl.is_empty() {
                nofallback += 1;
                continue;
            }
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("real-vs"),
                source: wgpu::ShaderSource::Wgsl(t.vs_wgsl.clone().into()),
            });
            if let Some(err) = block_on(device.pop_error_scope()) {
                bad += 1;
                if bad <= 5 {
                    eprintln!("INVALID VS {}#{i}: {err:?}", p.display());
                }
            } else {
                ok += 1;
            }
        }
    }
    eprintln!("real VS modules: {ok} valid, {bad} invalid, {nofallback} fell back");
    assert!(bad == 0, "{bad} VS modules produced invalid WGSL");
    assert!(ok > 0);
}

/// Renders offscreen to a PNG, so the output can be inspected without the live GUI.
#[test]
fn headless_render_png() {
    let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
        return;
    };
    let model_name = std::env::var("FF13_MODEL").unwrap_or_else(|_| "c001".into());
    let out = std::env::var("FF13_PNG_OUT").unwrap_or_else(|_| "/tmp/ff13_headless.png".into());
    let p = std::env::var("FF13_MODEL_PATH")
        .unwrap_or_else(|_| format!("{dir}/chr/pc/{model_name}/bin/{model_name}.win32.trb"));
    if !std::path::Path::new(&p).is_file() {
        return;
    }
    let model = super::read_model(std::path::Path::new(&p));
    if model.meshes.is_empty() {
        return;
    }

    let instance = wgpu::Instance::default();
    let Some(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();

    const W: u32 = 768;
    const H: u32 = 768;
    // Without MSAA, alpha-to-coverage cannot work and hair renders as opaque shards.
    const SAMPLES: u32 = 4;
    let tex_views: Vec<wgpu::TextureView> = model
        .textures
        .iter()
        .map(|t| make_view(&device, &queue, t))
        .collect();
    let defaults = [
        make_view(&device, &queue, &flat(&[210, 210, 214, 255])),
        make_view(&device, &queue, &flat(&[128, 128, 255, 255])),
        make_view(&device, &queue, &flat(&[28, 28, 28, 255])),
        make_view(&device, &queue, &flat(&[255, 255, 255, 255])),
        make_view(&device, &queue, &default_tone_ramp()),
    ];
    let rig = {
        let mut r = LightRig::default();
        if let Some(g) = std::env::var("FF13_GLOSS")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            r.gloss = g;
        }
        if let Some(a) = std::env::var("FF13_AMBIENT")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            r.ambient = a;
        }
        if let Some(k) = std::env::var("FF13_KEY").ok().and_then(|s| s.parse().ok()) {
            r.key_intensity = k;
        }
        if let Some(a) = std::env::var("FF13_AZIM").ok().and_then(|s| s.parse().ok()) {
            r.key_azim = a;
        }
        if let Some(e) = std::env::var("FF13_ELEV").ok().and_then(|s| s.parse().ok()) {
            r.key_elev = e;
        }
        r
    };
    let (_default_cube_tex, default_cube) = make_ambient_cube(&device, &queue, &rig);
    let (_reflect_cube_tex, reflect_cube) = make_reflect_cube(&device, &queue, rig.gloss, None);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let vs_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hl-vs"),
        size: std::mem::size_of::<VsU>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let (min, max) = bounds(&model.meshes);
    let mut cam = Camera::framing(min, max);
    if let Ok(y) = std::env::var("FF13_YAW") {
        cam.yaw = y.parse().unwrap_or(cam.yaw);
    }
    if let Ok(p) = std::env::var("FF13_PITCH") {
        cam.pitch = p.parse().unwrap_or(cam.pitch);
    }

    if let Ok(z) = std::env::var("FF13_ZOOM") {
        let mult = z.parse::<f32>().unwrap_or(0.32);
        // A fraction down from the top; around 0.55 frames the legs.
        let up = std::env::var("FF13_AIMY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(if mult < 0.2 { 0.11 } else { 0.18 });
        cam.target.y = max.y - (max.y - min.y) * up;
        cam.distance *= mult;
    }
    let view = cam.matrices(W as f32 / H as f32).0;
    let vsu = VsU {
        mvp: cam.view_proj(W as f32 / H as f32),
        eye: {
            let e = cam.eye();
            [e.x, e.y, e.z, 0.0]
        },
        view: view.to_cols_array_2d(),
        viewit: view.inverse().transpose().to_cols_array_2d(),
    };
    queue.write_buffer(&vs_buf, 0, bytemuck::bytes_of(&vsu));

    let mut reals: Vec<RealMat> = Vec::new();
    let mut real_idx: HashMap<String, Option<usize>> = HashMap::new();
    struct M {
        vbuf: wgpu::Buffer,
        ibuf: wgpu::Buffer,
        n: u32,
        real: Option<usize>,
        color: Option<usize>,
        normal: Option<usize>,
        bones: wgpu::BindGroup,
    }
    let bones_l = real_bones_layout(&device);
    // At the bind pose the skin is identity, so nothing visibly deforms without a pose.
    let skin: Option<Vec<Mat4>> = std::env::var("FF13_POSE").ok().and_then(|_| {
        model.skeleton.as_ref().map(|skel| {
            let mut rig = Rig::new(skel);
            if !rig.pose.is_empty() {
                rig.pose[0] = Vec3::new(0.0, 0.0, 0.9);
            }
            rig.solve().0
        })
    });
    let mut meshes = Vec::new();
    let only_mat = std::env::var("FF13_ONLY_MAT").unwrap_or_default();
    for m in model
        .meshes
        .iter()
        .filter(|m| !m.indices.is_empty() && m.tex.name.contains(&only_mat))
    {
        let color = m.tex.diffuse;
        let normal = m.tex.normal;
        let bones_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hl-bones"),
            contents: bytemuck::cast_slice(&palette_bytes(&m.palette, skin.as_deref())),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bones = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hl-bones-bg"),
            layout: &bones_l,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bones_buf.as_entire_binding(),
            }],
        });
        if std::env::var("FF13_DUMP_MAT").is_ok() {
            let nc = m
                .tex
                .ps
                .as_ref()
                .map(|p| p.constants.iter().filter(|c| c.reg_set == 2).count())
                .unwrap_or(0);
            eprintln!(
                "MAT {:<28} 2sided={} cutout={} diff={:?} norm={:?} spec={:?} dN={:?} G={:?} tone={:?} ps_consts={nc} samplers={:?}",
                m.tex.name,
                m.tex.two_sided,
                m.tex.cutout,
                m.tex.diffuse,
                m.tex.normal,
                m.tex.specular,
                m.tex.detail_normal,
                m.tex.opacity,
                m.tex.tone,
                m.tex.sampler_tex.keys().collect::<Vec<_>>(),
            );
        }
        let real = match &m.tex.ps {
            Some(ps) => *real_idx.entry(m.tex.name.clone()).or_insert_with(|| {
                build_real(
                    &device,
                    &tex_views,
                    &defaults,
                    &default_cube,
                    &reflect_cube,
                    &sampler,
                    &vs_buf,
                    &bones_l,
                    &m.tex.name,
                    ps,
                    m.tex.vs.as_ref(),
                    std::env::var("FF13_REAL_VS").is_ok(),
                    &rig,
                    &m.tex.sampler_tex,
                    &m.tex.color_samplers,
                    SAMPLES,
                    m.tex.cutout,
                )
                .map(|rm| {
                    reals.push(rm);
                    reals.len() - 1
                })
            }),
            None => None,
        };
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&m.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&m.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        meshes.push(M {
            vbuf,
            ibuf,
            n: m.indices.len() as u32,
            real,
            color,
            normal,
            bones,
        });
    }

    // Bypasses the game shaders entirely, to verify the normal pipeline itself.
    if std::env::var("FF13_DIAG").is_ok() {
        let diag_src = r#"
struct U { mvp: mat4x4<f32>, eye: vec4<f32>, light: vec4<f32>, };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var t_diff: texture_2d<f32>;
@group(1) @binding(1) var t_norm: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
struct VO { @builtin(position) pos: vec4<f32>, @location(0) n: vec3<f32>, @location(1) t: vec4<f32>, @location(2) uv: vec2<f32>, };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) nrm: vec3<f32>, @location(2) tan: vec4<f32>, @location(3) uv: vec2<f32>, @location(4) col: vec4<f32>) -> VO {
  var o: VO; o.pos = u.mvp * vec4<f32>(p, 1.0); o.n = nrm; o.t = tan; o.uv = vec2<f32>(uv.x, 1.0 - uv.y); return o;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  let d = textureSample(t_diff, samp, i.uv);
  let base = pow(d.rgb, vec3<f32>(2.2));
  let ng = normalize(i.n);
  let tg = normalize(i.t.xyz - ng * dot(ng, i.t.xyz));
  let b = cross(ng, tg) * i.t.w;
  var nm = textureSample(t_norm, samp, i.uv).xyz * 2.0 - 1.0;
  nm.z = sqrt(max(1.0 - nm.x * nm.x - nm.y * nm.y, 0.0));
  let n = normalize(tg * nm.x + b * nm.y + ng * nm.z);
  let l = normalize(-u.light.xyz);
  let lit = base * (max(dot(n, l), 0.0) * 0.9 + 0.18);
  return vec4<f32>(pow(max(lit, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)), 1.0);
}
"#;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("diag"),
            source: wgpu::ShaderSource::Wgsl(diag_src.into()),
        });
        let ubuf = |b, vis| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let g0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[ubuf(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let texl = |b| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let g1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                texl(0),
                texl(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&g0, &g1],
            push_constant_ranges: &[],
        });
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x4,3=>Float32x2,4=>Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: "vs",
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRS,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: "fs",
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        #[repr(C)]
        #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
        struct DU {
            mvp: [[f32; 4]; 4],
            eye: [f32; 4],
            light: [f32; 4],
        }
        let du = DU {
            mvp: cam.view_proj(W as f32 / H as f32),
            eye: {
                let e = cam.eye();
                [e.x, e.y, e.z, 0.0]
            },
            light: [-0.5, -0.7, -0.6, 0.0],
        };
        let dub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&du),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let dbg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &g0,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dub.as_entire_binding(),
            }],
        });
        let bgs: Vec<wgpu::BindGroup> = meshes
            .iter()
            .map(|m| {
                let dv = m
                    .color
                    .and_then(|i| tex_views.get(i))
                    .unwrap_or(&defaults[0]);
                let nv = m
                    .normal
                    .and_then(|i| tex_views.get(i))
                    .unwrap_or(&defaults[1]);
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &g1,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(dv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(nv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                })
            })
            .collect();
        let ct = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let cv = ct.create_view(&Default::default());
        let dt = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dv2 = dt.create_view(&Default::default());
        let rb = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cv,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &dbg0, &[]);
            for (m, bg) in meshes.iter().zip(&bgs) {
                pass.set_bind_group(1, bg, &[]);
                pass.set_vertex_buffer(0, m.vbuf.slice(..));
                pass.set_index_buffer(m.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.n, 0, 0..1);
            }
        }
        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &ct,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &rb,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(enc.finish()));
        rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let data = rb.slice(..).get_mapped_range();
        image::RgbaImage::from_raw(W, H, data.to_vec())
            .unwrap()
            .save(&out)
            .unwrap();
        eprintln!("wrote {out} (DIAG simple-light)");
        return;
    }

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hl-color-msaa"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let resolve = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hl-resolve"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let resolve_view = resolve.create_view(&Default::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hl-depth"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: SAMPLES,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&Default::default());

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hl-readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hl-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: Some(&resolve_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.05,
                        g: 0.05,
                        b: 0.07,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for m in &meshes {
            let Some(ri) = m.real else { continue };
            let rm = &reals[ri];
            pass.set_pipeline(&rm.pipeline);
            pass.set_bind_group(0, &rm.group0, &[]);
            pass.set_bind_group(1, &rm.group1, &[]);
            pass.set_bind_group(2, &m.bones, &[]);
            pass.set_vertex_buffer(0, m.vbuf.slice(..));
            pass.set_index_buffer(m.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.n, 0, 0..1);
        }
    }
    enc.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &resolve,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(enc.finish()));

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::Maintain::Wait);
    let data = readback.slice(..).get_mapped_range();
    // The target is sRGB, so the GPU already encoded these bytes. Encoding again here washes the
    // image out and hides real specular detail.
    let px: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(i, &b)| if i % 4 == 3 { 255 } else { b })
        .collect();
    image::RgbaImage::from_raw(W, H, px)
        .unwrap()
        .save(&out)
        .unwrap();
    eprintln!("wrote {out} ({} real materials)", reals.len());
}

#[test]
fn material_curve_reaches_vs_constant_register() {
    use ff13::formats::d3d9shader::{Constant, Shader};
    use ff13::formats::mot::{MaterialAnim, MaterialTrack};

    let vs = Shader {
        pixel: false,
        major: 3,
        minor: 0,
        insts: Vec::new(),
        constants: vec![
            Constant {
                name: "uvofs1".into(),
                reg_set: 2,
                reg_index: 7,
                reg_count: 1,
                default: Some(vec![0.0, 0.0, 0.0, 0.0]),
            },
            Constant {
                name: "shininess".into(),
                reg_set: 2,
                reg_index: 9,
                reg_count: 1,
                default: Some(vec![4.0, 0.0, 0.0, 0.0]),
            },
        ],
    };
    let anim = MaterialAnim {
        material: "m".into(),
        tracks: vec![MaterialTrack {
            constant: "uvofs1X".into(),
            register: "uvofs1".into(),
            component: Some(0),
            kind: 4,
            keys: Vec::new(),
            constant_value: Some(0.75),
        }],
    };
    let rig = LightRig::default();
    let overlay = anim.overlay_at(0.0);

    let plain = fill_vs_consts(&vs, 16, &rig, None);
    assert_eq!(
        plain[7],
        [0.0, 0.0, 0.0, 0.0],
        "unanimated uvofs1 keeps its CTAB default"
    );

    let animated = fill_vs_consts(&vs, 16, &rig, Some(&overlay));
    assert_eq!(
        animated[7],
        [0.75, 0.0, 0.0, 0.0],
        "curve drives lane 0 of register 7"
    );
    assert_eq!(
        animated[9], plain[9],
        "untouched constants keep their resolved value"
    );
}

/// A zero attenuation range divides by zero, and the resulting NaN blackens the material.
#[test]
fn sequel_point_light_constants_are_non_degenerate() {
    use ff13::formats::d3d9shader::Constant;

    let con = |name: &str, reg_index: u16| Constant {
        name: name.into(),
        reg_set: 2,
        reg_index,
        reg_count: 2,
        default: None,
    };
    let constants = vec![
        con("PointLightPositions", 0),
        con("PointLightParams", 2),
        con("PointLightColors", 4),
    ];
    let c = fill_consts(&constants, 8, &LightRig::default(), None);
    assert!(c.iter().flatten().all(|v| v.is_finite()));
    for (r, v) in c.iter().enumerate().take(4).skip(2) {
        assert!(v[2] != 0.0, "falloff exponent at c{r} is zero");
        assert!(
            v[3] != 0.0,
            "attenuation range at c{r} is zero (divides by zero)"
        );
    }
    for v in c.iter().take(6).skip(4) {
        assert_eq!(
            *v, [0.0; 4],
            "point lights are off, so their colour must be zero"
        );
    }
}

/// A permuted axis or a missing `1/pi` both survive a render, so neither shows up visually.
#[test]
fn sh_ambient_reconstructs_the_rig_ambient() {
    use ff13::formats::d3d9shader::Constant;

    let constants: Vec<Constant> = GRACE
        .iter()
        .enumerate()
        .map(|(i, n)| Constant {
            name: (*n).into(),
            reg_set: 2,
            reg_index: i as u16,
            reg_count: 1,
            default: None,
        })
        .collect();
    let rig = LightRig {
        ambient: 0.5,
        ambient_warmth: 0.0,
        ..Default::default()
    };
    let c = fill_consts(&constants, GRACE.len(), &rig, None);

    let irradiance = |n: Vec3| -> Vec3 {
        let (c1, c2, c3, c4, c5) = (0.429_043, 0.511_664, 0.743_125, 0.886_227, 0.247_708);
        let g = |i: usize| Vec3::new(c[i][0], c[i][1], c[i][2]);
        let (x, y, z) = (n.x, n.y, n.z);
        g(0) * c4 - g(6) * c5
            + g(6) * (c3 * z * z)
            + g(8) * (c1 * (x * x - y * y))
            + (g(4) * (x * y) + g(5) * (y * z) + g(7) * (x * z)) * (2.0 * c1)
            + (g(3) * x + g(1) * y + g(2) * z) * (2.0 * c2)
    };
    let (up, down, side) = (
        irradiance(Vec3::Y),
        irradiance(Vec3::NEG_Y),
        irradiance(Vec3::X),
    );

    assert!(
        up.x > down.x,
        "sky must be brighter than ground: {up:?} vs {down:?}"
    );
    assert!(
        (side.x - rig.ambient).abs() < 0.1,
        "sideways ambient {side:?} should sit at the rig level"
    );
    assert!(
        (0.5 * (up.x + down.x) - rig.ambient).abs() < 0.1,
        "mean ambient {up:?}/{down:?} off the rig level"
    );
    // Symmetric about the vertical axis, so only the `y` band-1 lane may carry a gradient.
    assert!(
        c[1][0].abs() > 0.02,
        "the sky/ground gradient must land in grace1_1"
    );
    assert!(
        c[2][0].abs() < 0.01 && c[3][0].abs() < 0.01,
        "grace10/grace11 must stay ~0"
    );
}

/// LR divides by pi in-shader where XIII-2 does not, so the two agree only up to that factor.
#[test]
fn lr_packed_sh_matches_the_xiii2_form() {
    use ff13::formats::d3d9shader::Constant;

    let con = |name: &str, reg_index: u16| Constant {
        name: name.into(),
        reg_set: 2,
        reg_index,
        reg_count: 1,
        default: None,
    };
    let rig = LightRig {
        ambient: 0.5,
        ambient_warmth: 0.3,
        ..Default::default()
    };
    let names: Vec<&str> = GRACE_LR.iter().chain(GRACE.iter()).copied().collect();
    let constants: Vec<Constant> = names
        .iter()
        .enumerate()
        .map(|(i, n)| con(n, i as u16))
        .collect();
    let c = fill_consts(&constants, names.len(), &rig, None);
    let reg = |n: &str| c[names.iter().position(|m| *m == n).unwrap()];

    let lr = |n: Vec3, ch: usize| -> f32 {
        let g0 = reg(["grace0r", "grace0g", "grace0b"][ch]);
        let g1 = reg(["grace1r", "grace1g", "grace1b"][ch]);
        let g2 = reg("grace2a");
        g0[0] * n.x
            + g0[1] * n.y
            + g0[2] * n.z
            + g0[3]
            + g1[0] * n.x * n.y
            + g1[1] * n.y * n.z
            + g1[2] * n.x * n.z
            + g1[3] * n.z * n.z
            + g2[ch] * (n.x * n.x - n.y * n.y)
    };
    let x2 = |n: Vec3, ch: usize| -> f32 {
        let (c1, c2, c3, c4, c5) = (0.429_043, 0.511_664, 0.743_125, 0.886_227, 0.247_708);
        let g = |name: &str| reg(name)[ch];
        let (x, y, z) = (n.x, n.y, n.z);
        g("grace00") * c4 - g("grace20") * c5
            + g("grace20") * (c3 * z * z)
            + g("grace22") * (c1 * (x * x - y * y))
            + (g("grace2_2") * x * y + g("grace2_1") * y * z + g("grace21") * x * z) * (2.0 * c1)
            + (g("grace11") * x + g("grace1_1") * y + g("grace10") * z) * (2.0 * c2)
    };

    let dirs = [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::X,
        Vec3::Z,
        Vec3::new(0.5, 0.6, -0.62).normalize(),
    ];
    for n in dirs {
        for ch in 0..3 {
            let (a, b) = (lr(n, ch), x2(n, ch) * std::f32::consts::PI);
            assert!(
                (a - b).abs() < 1e-4,
                "{n:?} channel {ch}: LR {a} vs XIII-2*pi {b}"
            );
        }
    }
    // A wrong lane order still balances at the poles.
    assert!((lr(Vec3::Y, 0) - lr(Vec3::NEG_Y, 0)).abs() > 0.05);
}

/// Works with a corpus root at either `chr` or `chr/pc`.
fn model_trbs(dir: &str) -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let Some(id) = p.file_name().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            let trb = p.join("bin").join(format!("{id}.win32.trb"));
            if trb.is_file() {
                out.push((id, trb));
            } else {
                stack.push(p);
            }
        }
    }
    out
}

#[test]
fn models_pull_textures_from_their_sibling_package() {
    let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
        return;
    };
    let (mut split, mut textured) = (0u32, 0u32);
    let mut bare: Vec<String> = Vec::new();
    for (id, p) in model_trbs(&dir) {
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        if ff13::formats::model::texture_packages(&bytes).is_empty() {
            continue;
        }
        // Some referenced packages are simply not in the shipped data.
        let on_disk = ff13::formats::model::texture_packages(&bytes)
            .iter()
            .all(|pkg| {
                p.parent()
                    .and_then(|b| b.parent())
                    .and_then(|m| m.parent())
                    .is_some_and(|root| {
                        root.join(pkg)
                            .join("bin")
                            .join(format!("{pkg}.win32.trb"))
                            .is_file()
                    })
            });
        if !on_disk {
            continue;
        }
        split += 1;
        let model = super::read_model(&p);
        if model.meshes.is_empty() {
            continue;
        }
        if model.textures.is_empty() {
            bare.push(id);
        } else {
            textured += 1;
        }
    }
    eprintln!("{textured}/{split} models needing a sibling package resolved textures");
    assert!(
        bare.is_empty(),
        "models left with no textures at all: {bare:?}"
    );
}
