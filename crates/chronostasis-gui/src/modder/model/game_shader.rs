use std::collections::HashMap;

use eframe::egui_wgpu::wgpu;
use ff13::formats::d3d9shader::{Constant, SamplerDim, Shader};
use ff13::formats::mot::ConstantOverlay;
use wgpu::util::DeviceExt;

use crate::modder::shader_transpile;

use ff13::formats::model::*;

use super::gpu::*;
use super::lighting::*;

/// In `Y(l,m)` order, with `_` marking negative m.
pub(crate) const GRACE: [&str; 9] = [
    "grace00", "grace1_1", "grace10", "grace11", "grace2_2", "grace2_1", "grace20", "grace21",
    "grace22",
];

pub(crate) const GRACE_LR: [&str; 7] = [
    "grace0r", "grace0g", "grace0b", "grace1r", "grace1g", "grace1b", "grace2a",
];

const SH_C: [f32; 5] = [0.429_043, 0.511_664, 0.743_125, 0.886_227, 0.247_708];

fn grace_lr(sh: &[[f32; 3]; 9], name: &str) -> Option<[f32; 4]> {
    let [c1, c2, c3, c4, c5] = SH_C;
    if name == "grace2a" {
        return Some([c1 * sh[8][0], c1 * sh[8][1], c1 * sh[8][2], 0.0]);
    }
    let ch = match name.as_bytes().last()? {
        b'r' => 0,
        b'g' => 1,
        b'b' => 2,
        _ => return None,
    };
    let l = |i: usize| sh[i][ch];
    match name.get(..6)? {
        "grace0" => Some([
            2.0 * c2 * l(3),
            2.0 * c2 * l(1),
            2.0 * c2 * l(2),
            c4 * l(0) - c5 * l(6),
        ]),
        "grace1" => Some([2.0 * c1 * l(4), 2.0 * c1 * l(5), 2.0 * c1 * l(7), c3 * l(6)]),
        _ => None,
    }
}

pub(crate) struct RealMat {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) group0: wgpu::BindGroup,
    pub(crate) group1: wgpu::BindGroup,
    pub(crate) const_buf: wgpu::Buffer,
    pub(crate) ps_constants: Vec<Constant>,
    pub(crate) const_count: usize,
    /// So per-material animation curves can be matched back to it.
    pub(crate) mat_name: String,
    pub(crate) vs_shader: Option<Shader>,
    pub(crate) vs_const_count: usize,
    pub(crate) vs_const_buf: Option<wgpu::Buffer>,
    _bool_buf: wgpu::Buffer,
    _vs_bufs: Vec<wgpu::Buffer>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct VsU {
    pub(crate) mvp: [[f32; 4]; 4],
    pub(crate) eye: [f32; 4],
    pub(crate) view: [[f32; 4]; 4],
    pub(crate) viewit: [[f32; 4]; 4],
}

pub(crate) fn real_bind_group_layouts(
    device: &wgpu::Device,
    t: &shader_transpile::Transpiled,
    vertex_consts: bool,
) -> (wgpu::BindGroupLayout, wgpu::BindGroupLayout) {
    let ubuf = |binding, vis| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut g0e = vec![
        ubuf(0, wgpu::ShaderStages::VERTEX),
        ubuf(1, wgpu::ShaderStages::FRAGMENT),
        ubuf(2, wgpu::ShaderStages::FRAGMENT),
    ];
    if vertex_consts {
        g0e.push(ubuf(3, wgpu::ShaderStages::VERTEX));
        g0e.push(ubuf(4, wgpu::ShaderStages::VERTEX));
    }
    let g0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ff13-real-g0"),
        entries: &g0e,
    });
    let mut entries = Vec::new();
    let mut b = 0u32;
    for (_, _, dim) in &t.samplers {
        let view_dimension = match dim {
            SamplerDim::Cube => wgpu::TextureViewDimension::Cube,
            SamplerDim::Volume => wgpu::TextureViewDimension::D3,
            _ => wgpu::TextureViewDimension::D2,
        };
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        b += 2;
    }
    let g1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ff13-real-g1"),
        entries: &entries,
    });
    (g0, g1)
}

pub(crate) fn real_bones_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ff13-real-bones"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_real_pipeline(
    device: &wgpu::Device,
    g0: &wgpu::BindGroupLayout,
    g1: &wgpu::BindGroupLayout,
    bones: &wgpu::BindGroupLayout,
    vs_mod: &wgpu::ShaderModule,
    fs_mod: &wgpu::ShaderModule,
    samples: u32,
    cutout: bool,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ff13-real-pl"),
        bind_group_layouts: &[g0, g1, bones],
        push_constant_ranges: &[],
    });
    // The bound attributes are not a contiguous prefix of `Vertex`.
    let va = |format, field, shader_location| wgpu::VertexAttribute {
        format,
        offset: field,
        shader_location,
    };
    let attrs = [
        va(
            wgpu::VertexFormat::Float32x3,
            std::mem::offset_of!(Vertex, pos) as u64,
            0,
        ),
        va(
            wgpu::VertexFormat::Float32x3,
            std::mem::offset_of!(Vertex, normal) as u64,
            1,
        ),
        va(
            wgpu::VertexFormat::Float32x4,
            std::mem::offset_of!(Vertex, tangent) as u64,
            2,
        ),
        va(
            wgpu::VertexFormat::Float32x2,
            std::mem::offset_of!(Vertex, uv) as u64,
            3,
        ),
        va(
            wgpu::VertexFormat::Float32x4,
            std::mem::offset_of!(Vertex, color) as u64,
            4,
        ),
        va(
            wgpu::VertexFormat::Float32x2,
            std::mem::offset_of!(Vertex, uv1) as u64,
            5,
        ),
        va(
            wgpu::VertexFormat::Uint32x4,
            std::mem::offset_of!(Vertex, joints) as u64,
            6,
        ),
        va(
            wgpu::VertexFormat::Float32x4,
            std::mem::offset_of!(Vertex, weights) as u64,
            7,
        ),
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ff13-real-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: vs_mod,
            entry_point: "vs_main",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: fs_mod,
            entry_point: "fs_main",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::COLOR,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
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
        multisample: wgpu::MultisampleState {
            count: samples,
            // With a2c on a non-cutout material, blend alpha becomes screen-door holes.
            alpha_to_coverage_enabled: samples > 1 && cutout,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

pub(crate) fn fill_consts(
    constants: &[Constant],
    n: usize,
    rig: &LightRig,
    overlay: Option<&ConstantOverlay>,
) -> Vec<[f32; 4]> {
    let mut c = vec![[0.0f32; 4]; n];
    let sh = rig.sh9();
    // These shaders carry ambient as SH, so `ambientColor` would count it twice.
    let sh_ambient = constants
        .iter()
        .any(|c| GRACE.contains(&c.name.as_str()) || GRACE_LR.contains(&c.name.as_str()));
    let set = |c: &mut Vec<[f32; 4]>, con: &ff13::formats::d3d9shader::Constant, v: [f32; 4]| {
        for r in 0..con.reg_count as usize {
            if let Some(slot) = c.get_mut(con.reg_index as usize + r) {
                *slot = v;
            }
        }
    };
    // Slot 0 only: these are `float4[4]` arrays but only index 0 is ever read, and splatting the
    // key across all four blows out specular.
    let set_first =
        |c: &mut Vec<[f32; 4]>, con: &ff13::formats::d3d9shader::Constant, v: [f32; 4]| {
            if let Some(slot) = c.get_mut(con.reg_index as usize) {
                *slot = v;
            }
        };
    for con in constants {
        if con.reg_set != 2 {
            continue;
        }
        if let Some(d) = &con.default {
            for r in 0..con.reg_count as usize {
                if let Some(slot) = c.get_mut(con.reg_index as usize + r) {
                    for (k, item) in slot.iter_mut().enumerate() {
                        *item = d.get(r * 4 + k).copied().unwrap_or(0.0);
                    }
                }
            }
        }
        match con.name.as_str() {
            "lightDirections" | "DirLightDirections" => set_first(&mut c, con, rig.dir()),
            "lightColors" | "DirLightColors" => set_first(&mut c, con, rig.key_color()),
            "lightPositions" => {
                let d = rig.dir();
                set_first(
                    &mut c,
                    con,
                    [d[0] * -1000.0, d[1] * -1000.0, d[2] * -1000.0, 1.0],
                );
            }
            // Unbound, a zero range makes the attenuation infinite and the colour NaN.
            "PointLightPositions" => set(&mut c, con, [0.0, 1.0e4, 0.0, 1.0]),
            "PointLightParams" => set(&mut c, con, [1.0, 1.0, 1.0, 1.0]),
            "PointLightColors" => set(&mut c, con, [0.0; 4]),
            "ambientColor" | "ambientLightColor" if sh_ambient => {}
            // No CTAB default, and it scales the whole SH result, so unbound leaves ambient black.
            "envMapColor" => set(&mut c, con, [1.0, 1.0, 1.0, 1.0]),
            "ambientColor" => set(&mut c, con, rig.ambient_color()),
            "ambientLightColor" => {
                set(&mut c, con, env4("FF13_ALC").unwrap_or(rig.ambient_color()))
            }
            // Not `[1,1,1,1]`: equal wrap bounds divide by zero and speckle skin.
            "lightParams" => set(&mut c, con, env4("FF13_LP").unwrap_or([1.0, 1.0, 1.0, 1.0])),
            "latitudeParam" => set(
                &mut c,
                con,
                env4("FF13_LAT").unwrap_or([1.0, 0.0, 0.75, 1.0]),
            ),
            // Eyes are reflection-dominant, so zeroing this renders them near-black while every
            // other material looks fine. The CTAB default is already the per-material value.
            "specularColor" => {
                if let Some(v) = std::env::var("FF13_SPEC")
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                {
                    set(&mut c, con, [v, v, v, 0.0]);
                }
            }
            "shadowSplitRange" => set(&mut c, con, [1.0e9, 0.0, 0.0, 0.0]),
            "ambientCubeMapPower" => {
                let v = std::env::var("FF13_ACMP")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1.0);
                set(&mut c, con, [v, 0.0, 0.0, 0.0]);
            }
            // `drawEnvBlendRatio` selects between the dynamic and static reflection rather than
            // scaling either, and both samplers here are the same cube, so it is a no-op.
            // The synthesized VS carries no fog coord, and reading `UV.w` blackens hair.
            "fogColor" => set(
                &mut c,
                con,
                env4("FF13_FOG").unwrap_or([0.5, 0.5, 0.5, 0.0]),
            ),
            // XIII-2 never divides by pi, and there is no CTAB default to inherit.
            _ if GRACE.contains(&con.name.as_str()) => {
                let i = GRACE.iter().position(|g| *g == con.name).unwrap_or(0);
                let v = sh[i].map(|x| x / std::f32::consts::PI);
                set(&mut c, con, [v[0], v[1], v[2], 0.0]);
            }
            _ if GRACE_LR.contains(&con.name.as_str()) => {
                if let Some(v) = grace_lr(&sh, &con.name) {
                    set(&mut c, con, v);
                }
            }
            _ => {}
        }
    }
    apply_overlay(&mut c, constants, overlay);
    c
}

/// Only the lanes the curves actually drive; everything else keeps its resolved value.
fn apply_overlay(c: &mut [[f32; 4]], constants: &[Constant], overlay: Option<&ConstantOverlay>) {
    let Some(ov) = overlay.filter(|o| !o.is_empty()) else {
        return;
    };
    for con in constants.iter().filter(|c| c.reg_set == 2) {
        if let Some(slot) = c.get_mut(con.reg_index as usize) {
            ov.apply(&con.name, slot);
        }
    }
}

pub(crate) fn fill_vs_consts(
    vs: &Shader,
    n: usize,
    rig: &LightRig,
    overlay: Option<&ConstantOverlay>,
) -> Vec<[f32; 4]> {
    let mut c = vec![[0.0f32; 4]; n];
    let consts: Vec<_> = vs.constants.iter().filter(|c| c.reg_set == 2).collect();
    for con in &consts {
        if let Some(d) = &con.default {
            for r in 0..con.reg_count as usize {
                if let Some(slot) = c.get_mut(con.reg_index as usize + r) {
                    for (k, item) in slot.iter_mut().enumerate() {
                        *item = d.get(r * 4 + k).copied().unwrap_or(0.0);
                    }
                }
            }
        }
    }
    let ident = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for con in &consts {
        let name = con.name.to_ascii_lowercase();
        let base = con.reg_index as usize;
        if name.contains("worldit") || name == "worldmatrix" {
            for (r, row) in ident.iter().enumerate().take(con.reg_count as usize) {
                if let Some(slot) = c.get_mut(base + r) {
                    *slot = *row;
                }
            }
        } else if name == "modelbboxscale" {
            if let Some(slot) = c.get_mut(base) {
                *slot = [1.0, 1.0, 1.0, 1.0];
            }
        } else if name == "modelbboxoffset"
            && let Some(slot) = c.get_mut(base)
        {
            *slot = [0.0, 0.0, 0.0, 0.0];
        }
    }
    // A zero wrap range divides by zero and blows out the COLOR varying.
    for con in &consts {
        let base = con.reg_index as usize;
        match con.name.as_str() {
            "lightDirections" | "DirLightDirections" => {
                if let Some(s) = c.get_mut(base) {
                    *s = rig.dir();
                }
            }
            "lightColors" | "DirLightColors" => {
                if let Some(s) = c.get_mut(base) {
                    *s = rig.key_color();
                }
            }
            "lightPositions" => {
                let d = rig.dir();
                if let Some(s) = c.get_mut(base) {
                    *s = [d[0] * -1000.0, d[1] * -1000.0, d[2] * -1000.0, 1.0];
                }
            }
            "lightParams" => {
                let v = env4("FF13_LP").unwrap_or([0.371, 0.405, 0.408, 1.770]);
                for r in 0..con.reg_count as usize {
                    if let Some(s) = c.get_mut(base + r) {
                        *s = v;
                    }
                }
            }
            "ambientColor" | "ambientLightColor" => {
                for r in 0..con.reg_count as usize {
                    if let Some(s) = c.get_mut(base + r) {
                        *s = rig.ambient_color();
                    }
                }
            }
            "specularColor" => {
                if let Some(s) = c.get_mut(base) {
                    *s = [0.0, 0.0, 0.0, 0.0];
                }
            }
            _ => {}
        }
    }
    apply_overlay(&mut c, &vs.constants, overlay);
    c
}

pub(crate) fn consts_with_gamma(
    mut c: Vec<[f32; 4]>,
    const_count: usize,
    gamma: f32,
) -> Vec<[f32; 4]> {
    c.resize(const_count.max(1), [0.0; 4]);
    c.push([gamma, 0.0, 0.0, 0.0]);
    c
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_real(
    device: &wgpu::Device,
    tex_views: &[wgpu::TextureView],
    defaults: &[wgpu::TextureView; 5],
    default_cube: &wgpu::TextureView,
    reflect_cube: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    vs_buf: &wgpu::Buffer,
    bones_layout: &wgpu::BindGroupLayout,
    mat_name: &str,
    ps: &Shader,
    vs: Option<&Shader>,
    real_vs: bool,
    rig: &LightRig,
    sampler_tex: &HashMap<String, usize>,
    color_samplers: &std::collections::HashSet<String>,
    samples: u32,
    cutout: bool,
) -> Option<RealMat> {
    let color_regs: std::collections::BTreeSet<u16> = ps
        .constants
        .iter()
        .filter(|c| c.reg_set == 3 && color_samplers.contains(&c.name))
        .map(|c| c.reg_index)
        .collect();
    let vs = vs.filter(|_| real_vs);
    let t = match shader_transpile::full_module(ps, &color_regs, vs) {
        Ok(t) => t,
        Err(e) => {
            if std::env::var("FF13_DEBUG").is_ok() {
                eprintln!("  transpile FAILED: {e}");
            }
            return None;
        }
    };
    let real_vs = !t.vs_wgsl.is_empty();
    if real_vs && std::env::var("FF13_DUMP_VS").is_ok() {
        eprintln!("===VS===\n{}\n===END===", t.vs_wgsl);
    }
    if std::env::var("FF13_DUMP_PS").is_ok() {
        let skin = ps.constants.iter().any(|c| c.name == "lightToneMap");
        eprintln!("===PS skin={skin} float-consts===");
        for c in ps.constants.iter().filter(|c| c.reg_set == 2) {
            let d = c
                .default
                .as_ref()
                .map(|v| format!("default={:?}", &v[..v.len().min(4)]))
                .unwrap_or_default();
            eprintln!(
                "  c{:<4} x{:<2} {:<24} {d}",
                c.reg_index, c.reg_count, c.name
            );
        }
        eprintln!("  -- bools --");
        for c in ps.constants.iter().filter(|c| c.reg_set == 0) {
            eprintln!("  b{:<3} {}", c.reg_index, c.name);
        }
        eprintln!("  -- shader samplers (bind order) --");
        for (i, (reg, name, dim)) in t.samplers.iter().enumerate() {
            let bind = if *dim == SamplerDim::Cube {
                "CUBE-stub".to_string()
            } else if let Some(ti) = sampler_tex.get(name) {
                format!("tex{ti}")
            } else if name == "lightToneMap" {
                "tone-stub".to_string()
            } else if name.contains("shadowMap") {
                "shadow-stub".to_string()
            } else {
                "DEFAULT-stub".to_string()
            };
            eprintln!("  t{i} reg=s{reg} {name:<18} {dim:?} -> {bind}");
        }
        if std::env::var("FF13_DUMP_PS_WGSL").is_ok() {
            eprintln!("---WGSL---\n{}\n---END---", t.wgsl);
        }
    }
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ff13-real-vs"),
        source: wgpu::ShaderSource::Wgsl(
            if real_vs {
                t.vs_wgsl.clone()
            } else {
                t.wgsl.clone()
            }
            .into(),
        ),
    });
    let fs_module = real_vs.then(|| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ff13-real-fs"),
            source: wgpu::ShaderSource::Wgsl(t.wgsl.clone().into()),
        })
    });
    let fs = fs_module.as_ref().unwrap_or(&module);
    let (g0l, g1l) = real_bind_group_layouts(device, &t, real_vs);
    let pipeline = build_real_pipeline(
        device,
        &g0l,
        &g1l,
        bones_layout,
        &module,
        fs,
        samples,
        cutout,
    );
    let consts = fill_consts(&ps.constants, t.const_count, rig, None);
    let mut bools = [0u32; 16];
    if let Some(mask) = std::env::var("FF13_PS_BOOLS")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        for (i, slot) in bools.iter_mut().enumerate() {
            *slot = (mask >> i) & 1;
        }
    }
    let consts = consts_with_gamma(consts, t.const_count, rig.gamma_exp);
    let const_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ff13-real-consts"),
        contents: bytemuck::cast_slice(&consts),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bool_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ff13-real-bools"),
        contents: bytemuck::cast_slice(&bools),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let vk_buf = vs.filter(|_| real_vs).map(|vs| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ff13-real-vs-consts"),
            contents: bytemuck::cast_slice(&fill_vs_consts(vs, t.vs_const_count, rig, None)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })
    });
    // `b0` selects palette skinning; the other bools run alternate paths with garbage inputs.
    let vbool_buf = real_vs.then(|| {
        let mut bools = [0u32; 16];
        bools[0] = 1;
        if let Some(mask) = std::env::var("FF13_VS_BOOLS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        {
            for (i, b) in bools.iter_mut().enumerate() {
                *b = (mask >> i) & 1;
            }
        }
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ff13-real-vs-bools"),
            contents: bytemuck::cast_slice(&bools),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    });
    let mut g0_entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: vs_buf.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: const_buf.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: bool_buf.as_entire_binding(),
        },
    ];
    if let (Some(vk), Some(vb)) = (&vk_buf, &vbool_buf) {
        g0_entries.push(wgpu::BindGroupEntry {
            binding: 3,
            resource: vk.as_entire_binding(),
        });
        g0_entries.push(wgpu::BindGroupEntry {
            binding: 4,
            resource: vb.as_entire_binding(),
        });
    }
    let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ff13-real-g0"),
        layout: &g0l,
        entries: &g0_entries,
    });

    let mut entries = Vec::new();
    let mut b = 0u32;
    for (_reg, name, dim) in &t.samplers {
        let view: &wgpu::TextureView = if let Some(view) = sampler_tex
            .get(name)
            .and_then(|&ti| tex_views.get(ti))
            .filter(|_| *dim != SamplerDim::Cube)
        {
            view
        } else if *dim == SamplerDim::Cube {
            if name == "ambientCubeMap" {
                default_cube
            } else {
                reflect_cube
            }
        } else if name == "lightToneMap" {
            &defaults[4]
        } else if name.contains("shadowMap") {
            &defaults[3]
        } else {
            &defaults[0]
        };
        entries.push(wgpu::BindGroupEntry {
            binding: b,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: b + 1,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
        b += 2;
    }
    let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ff13-real-g1"),
        layout: &g1l,
        entries: &entries,
    });

    if let Some(err) = block_on(device.pop_error_scope()) {
        eprintln!("real shader pipeline rejected: {err:?}");
        return None;
    }
    let _vs_bufs: Vec<wgpu::Buffer> = vbool_buf.into_iter().collect();
    Some(RealMat {
        pipeline,
        group0,
        group1,
        const_buf,
        ps_constants: ps.constants.clone(),
        const_count: t.const_count,
        mat_name: mat_name.to_string(),
        vs_shader: vs.filter(|_| real_vs).cloned(),
        vs_const_count: t.vs_const_count,
        vs_const_buf: vk_buf,
        _bool_buf: bool_buf,
        _vs_bufs,
    })
}
