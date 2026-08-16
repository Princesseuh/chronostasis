use eframe::egui_wgpu::wgpu;
use glam::Vec3;

/// Eye and cornea materials render as almost pure environment reflection, so a flat cube leaves
/// them black with no catchlight.
const REFLECT_CUBE_SIZE: u32 = 256;

/// Faces are ordered `+x -x +y -y +z -z`.
fn cube_dir(face: usize, x: u32, y: u32, size: u32) -> Vec3 {
    let u = 2.0 * (x as f32 + 0.5) / size as f32 - 1.0;
    let v = 2.0 * (y as f32 + 0.5) / size as f32 - 1.0;
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

/// A stylized stand-in for the game's per-frame dynamic scene cube, which exists in no file. Real
/// baked probes are made for set geometry and look wrong on characters.
fn studio_env(dir: Vec3, brightness: f32) -> Vec3 {
    let sky = Vec3::new(0.80, 0.86, 1.00);
    let horizon = Vec3::new(0.48, 0.50, 0.56);
    let ground = Vec3::new(0.16, 0.15, 0.15);
    let mut c = if dir.y >= 0.0 {
        horizon.lerp(sky, dir.y.powf(0.6))
    } else {
        horizon.lerp(ground, (-dir.y).powf(0.5))
    };
    // A broad glow plus a tight core, so eyes get a crisp catchlight.
    let key = Vec3::new(-0.35, 0.45, 1.0).normalize();
    let k = dir.dot(key).max(0.0);
    c += Vec3::splat(0.6 * k.powf(6.0) + 2.5 * k.powf(400.0));
    c * brightness
}

/// Face-major, then row-major.
fn studio_cube_faces(size: u32, brightness: f32) -> Vec<u8> {
    let mut data = Vec::with_capacity((size * size * 6 * 4) as usize);
    let q = |f: f32| (f * 255.0).round().clamp(0.0, 255.0) as u8;
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let c = studio_env(cube_dir(face, x, y, size), brightness);
                data.extend_from_slice(&[q(c.x), q(c.y), q(c.z), 255]);
            }
        }
    }
    data
}

fn studio_chain(gloss: f32) -> Vec<(u32, Vec<u8>)> {
    let brightness = std::env::var("FF13_REFLECT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(gloss);
    let mut chain = Vec::new();
    let mut size = REFLECT_CUBE_SIZE;
    while size >= 4 {
        chain.push((size, studio_cube_faces(size, brightness)));
        size /= 2;
    }
    chain
}

pub(crate) fn cube_texture(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ff13-real-cube"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

pub(crate) fn write_cube_faces(queue: &wgpu::Queue, tex: &wgpu::Texture, faces: &[[u8; 4]; 6]) {
    let data: Vec<u8> = faces.iter().flatten().copied().collect();
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
    );
}

pub(crate) fn cube_view(tex: &wgpu::Texture) -> wgpu::TextureView {
    tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// Shared by the ambient cube and the SH projection, so the two cannot drift apart.
pub(crate) fn env_radiance(rig: &LightRig, d: Vec3) -> Vec3 {
    let kd = rig.dir();
    let to_light = -Vec3::new(kd[0], kd[1], kd[2]);
    let kc = rig.key_color();
    let key = Vec3::new(kc[0], kc[1], kc[2]);
    let sky = Vec3::new(0.38, 0.42, 0.48);
    let ground = Vec3::new(0.30, 0.27, 0.26);
    let strength: f32 = std::env::var("FF13_CUBEKEY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    ground.lerp(sky, (d.y + 1.0) * 0.5) + key * (d.dot(to_light).max(0.0) * strength)
}

/// No key lobe, since the shaders light the key separately.
fn ambient_radiance(rig: &LightRig, d: Vec3) -> Vec3 {
    let sky = Vec3::new(0.38, 0.42, 0.48);
    let ground = Vec3::new(0.30, 0.27, 0.26);
    let a = rig.ambient_color();
    ground.lerp(sky, (d.y + 1.0) * 0.5) / ((sky + ground) * 0.5) * Vec3::new(a[0], a[1], a[2])
}

pub(crate) fn ambient_cube_faces(rig: &LightRig) -> [[u8; 4]; 6] {
    let dirs = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    dirs.map(|f| {
        let c = env_radiance(rig, f);
        let q = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
        [q(c.x), q(c.y), q(c.z), 255]
    })
}

/// In the order XIII-2 names its `grace` constants, which is `Y(l,m)` with `_` standing in for -m.
fn sh_basis(d: Vec3) -> [f32; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// Plain radiance coefficients: LR takes these as-is, dividing in-shader, so only XIII-2's binding
/// has to pre-divide.
pub(crate) fn sh9(rig: &LightRig) -> [[f32; 3]; 9] {
    const N: usize = 2048;
    let mut acc = [Vec3::ZERO; 9];
    // A Fibonacci sphere spreads evenly with no polar clustering.
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    for i in 0..N {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / N as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * i as f32;
        let d = Vec3::new(theta.cos() * r, y, theta.sin() * r);
        let rad = ambient_radiance(rig, d);
        for (a, b) in acc.iter_mut().zip(sh_basis(d)) {
            *a += rad * b;
        }
    }
    let w = 4.0 * std::f32::consts::PI / N as f32;
    acc.map(|v| {
        let c = v * w;
        [c.x, c.y, c.z]
    })
}

pub(crate) fn make_ambient_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rig: &LightRig,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = cube_texture(device);
    write_cube_faces(queue, &tex, &ambient_cube_faces(rig));
    let view = cube_view(&tex);
    (tex, view)
}

fn dds_face(dds: &[u8]) -> Option<image::RgbaImage> {
    let parsed = ddsfile::Dds::read(std::io::Cursor::new(dds)).ok()?;
    let img = image_dds::image_from_dds(&parsed, 0).ok()?;
    image::RgbaImage::from_raw(img.width(), img.height(), img.into_raw())
}

/// The whole chain is downsampled from a single base, so every mip is a monotonic reduction of the
/// same image. Mixing the source's own small mips with an upscaled base gave a non-monotonic chain,
/// which made the cube LOD jump across a curved eye and tear the reflection.
fn load_env_cube(trb_path: &std::path::Path) -> Option<Vec<(u32, Vec<u8>)>> {
    use ff13::formats::{imgb, trb::Trb};
    use image::imageops::{FilterType::Triangle, resize};
    let header = std::fs::read(trb_path).ok()?;
    let imgb_bytes = std::fs::read(trb_path.with_extension("imgb")).ok()?;
    let trb = Trb::parse(&header).ok()?;
    for res in trb.texture_resources() {
        let Some(d) = trb.resource_data(res) else {
            continue;
        };
        for o in imgb::find_gtex(d) {
            let Ok(g) = imgb::parse_gtex(d, o) else {
                continue;
            };
            let Some(levels) = imgb::extract_cubemap_mips(&g, &imgb_bytes) else {
                continue;
            };
            // The largest source mip's faces, resized to the fixed cube size.
            let (tw, _, top) = &levels[0];
            let mut base = Vec::with_capacity(6);
            for dds in top {
                let img = dds_face(dds)?;
                base.push(if *tw as u32 == REFLECT_CUBE_SIZE {
                    img
                } else {
                    resize(&img, REFLECT_CUBE_SIZE, REFLECT_CUBE_SIZE, Triangle)
                });
            }
            let mut chain = Vec::new();
            let mut size = REFLECT_CUBE_SIZE;
            while size >= 4 {
                let mut data = Vec::with_capacity((size * size * 4 * 6) as usize);
                for f in &base {
                    if size == REFLECT_CUBE_SIZE {
                        data.extend_from_slice(f.as_raw());
                    } else {
                        data.extend_from_slice(resize(f, size, size, Triangle).as_raw());
                    }
                }
                chain.push((size, data));
                size /= 2;
            }
            return Some(chain);
        }
    }
    None
}

/// Always the same base and level count, so switching cubes is an in-place rewrite.
///
/// Reflective materials add the environment as a signed delta around mid-gray, so a cube darker
/// than 0.5 subtracts and a brighter one adds; the cube's absolute level is what shows up. Real
/// baked probes measure dark, which is why the game's own math mostly darkens plus catchlights.
///
/// The default is the stylized stand-in, since it reads well on eyes; `FF13_ENVCUBE` feeds a raw
/// file cube instead.
fn reflect_cube_data(gloss: f32, env_cube: Option<&std::path::Path>) -> Vec<(u32, Vec<u8>)> {
    let from_env = std::env::var_os("FF13_ENVCUBE").map(std::path::PathBuf::from);
    let mut chain = if let Some(p) = env_cube.or(from_env.as_deref()) {
        match load_env_cube(p) {
            Some(c) if !c.is_empty() => c,
            _ => {
                eprintln!("env cube: no loadable cubemap in {p:?}");
                studio_chain(gloss)
            }
        }
    } else {
        studio_chain(gloss)
    };
    center_reflection(&mut chain);
    chain
}

/// Non-physical and off by default: the game never re-centers, so this is purely a look knob for
/// when a real cube's raw level is too dark or bright for taste.
fn center_reflection(chain: &mut [(u32, Vec<u8>)]) {
    if std::env::var("FF13_REFLECT_CENTER").as_deref() != Ok("1") {
        return;
    }
    let contrast: f32 = std::env::var("FF13_REFLECT_CONTRAST")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.55);
    // Over the base level, which every other level is a reduction of.
    let Some((_, base)) = chain.first() else {
        return;
    };
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for px in base.chunks_exact(4) {
        let (r, g, b) = (
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        sum += (0.299 * r + 0.587 * g + 0.114 * b) as f64;
        n += 1;
    }
    if n == 0 {
        return;
    }
    let mean = (sum / n as f64) as f32;
    // The same shift on every channel preserves colour differences.
    for (_, data) in chain.iter_mut() {
        for px in data.chunks_exact_mut(4) {
            for c in &mut px[..3] {
                let v = *c as f32 / 255.0;
                let out = ((v - mean) * contrast + 0.5).clamp(0.0, 1.0);
                *c = (out * 255.0).round() as u8;
            }
        }
    }
}

fn write_cube_levels(queue: &wgpu::Queue, tex: &wgpu::Texture, levels: &[(u32, Vec<u8>)]) {
    for (level, (size, data)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * size),
                rows_per_image: Some(*size),
            },
            wgpu::Extent3d {
                width: *size,
                height: *size,
                depth_or_array_layers: 6,
            },
        );
    }
}

/// Reuses the existing texture, which is why base and level count never change.
pub(crate) fn write_reflect_cube(
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    gloss: f32,
    env_cube: Option<&std::path::Path>,
) {
    write_cube_levels(queue, tex, &reflect_cube_data(gloss, env_cube));
}

pub(crate) fn make_reflect_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gloss: f32,
    env_cube: Option<&std::path::Path>,
) -> (wgpu::Texture, wgpu::TextureView) {
    let levels = reflect_cube_data(gloss, env_cube);
    let size = levels[0].0;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ff13-real-reflect-cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_cube_levels(queue, &tex, &levels);
    let view = cube_view(&tex);
    (tex, view)
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct LightRig {
    pub(crate) key_azim: f32,
    pub(crate) key_elev: f32,
    pub(crate) key_intensity: f32,
    pub(crate) key_warmth: f32,
    pub(crate) ambient: f32,
    pub(crate) ambient_warmth: f32,
    pub(crate) gloss: f32,
    pub(crate) gamma_exp: f32,
}

impl Default for LightRig {
    fn default() -> Self {
        Self {
            key_azim: -2.51,
            key_elev: 0.97,
            key_intensity: 1.0,
            key_warmth: 0.28,
            // Measured from the live game: fill illumination is around 0.36-0.39, and anything
            // near half that is the main cause of a bleak-looking render.
            ambient: 0.37,
            ambient_warmth: 0.10,
            gloss: 1.0,
            gamma_exp: 1.0,
        }
    }
}

impl LightRig {
    pub(crate) fn dir(&self) -> [f32; 4] {
        let (se, ce) = (self.key_elev.sin(), self.key_elev.cos());
        let (sa, ca) = (self.key_azim.sin(), self.key_azim.cos());
        let v = Vec3::new(ce * sa, -se, ce * ca).normalize();
        [v.x, v.y, v.z, 0.0]
    }
    pub(crate) fn key_color(&self) -> [f32; 4] {
        tint(self.key_intensity, self.key_warmth)
    }
    pub(crate) fn ambient_color(&self) -> [f32; 4] {
        tint(self.ambient, self.ambient_warmth)
    }

    pub(crate) fn sh9(&self) -> [[f32; 3]; 9] {
        sh9(self)
    }
}

pub(crate) fn tint(level: f32, warmth: f32) -> [f32; 4] {
    let r = level * (1.0 - (-warmth).max(0.0) * 0.25);
    let b = level * (1.0 - warmth.max(0.0) * 0.25);
    let g = level * (1.0 - warmth.abs() * 0.06);
    [r, g, b, 1.0]
}

pub(crate) fn env4(name: &str) -> Option<[f32; 4]> {
    let v: Vec<f32> = std::env::var(name)
        .ok()?
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}
