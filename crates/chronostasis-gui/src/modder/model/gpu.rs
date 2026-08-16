use std::collections::{HashMap, HashSet};

use eframe::egui_wgpu::{self, wgpu};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use ff13::formats::model::*;

use super::game_shader::*;
use super::lighting::*;
use super::rig::*;

pub(crate) struct Camera {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) distance: f32,
    pub(crate) target: Vec3,
    pub(crate) radius: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera::framing(Vec3::splat(-1.0), Vec3::splat(1.0))
    }
}

impl Camera {
    pub(crate) fn framing(min: Vec3, max: Vec3) -> Self {
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(0.001);
        Camera {
            yaw: 0.7,
            pitch: 0.3,
            distance: radius * 2.8,
            target: center,
            radius,
        }
    }

    pub(crate) fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        -Vec3::new(cp * sy, sp, cp * cy)
    }

    pub(crate) fn eye(&self) -> Vec3 {
        self.target - self.forward() * self.distance
    }

    pub(crate) fn pan(&mut self, dx: f32, dy: f32) {
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(fwd).normalize_or_zero();
        let scale = self.distance * 0.0015;
        self.target += right * (-dx * scale) + up * (dy * scale);
    }

    pub(crate) fn matrices(&self, aspect: f32) -> (Mat4, Mat4) {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(
            55f32.to_radians(),
            aspect.max(0.01),
            (self.radius * 0.02).max(1e-4),
            self.distance + self.radius * 6.0,
        );
        (view, proj * view)
    }

    pub(crate) fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        self.matrices(aspect).1.to_cols_array_2d()
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) light_dir: [f32; 4],
    pub(crate) eye: [f32; 4],
    pub(crate) uv_xform: [f32; 4],
    pub(crate) flags: [f32; 4],
    pub(crate) diffuse_mask: [f32; 4],
    pub(crate) normal_mask: [f32; 4],
    pub(crate) specular_mask: [f32; 4],
    pub(crate) opacity_mask: [f32; 4],
    pub(crate) tone_mask: [f32; 4],
    pub(crate) map_on: [f32; 4],
    pub(crate) map_on2: [f32; 4],
}

pub(crate) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub(crate) const MSAA: u32 = 4;

pub(crate) fn shader_source() -> String {
    SHADER.replace("MAX_PALETTE", &MAX_PALETTE.to_string())
}

pub(crate) const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
    uv_xform: vec4<f32>,
    flags: vec4<f32>,
    diffuse_mask: vec4<f32>,
    normal_mask: vec4<f32>,
    specular_mask: vec4<f32>,
    opacity_mask: vec4<f32>,
    tone_mask: vec4<f32>,
    map_on: vec4<f32>,
    map_on2: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var t_normal: texture_2d<f32>;
@group(1) @binding(2) var t_specular: texture_2d<f32>;
@group(1) @binding(3) var samp: sampler;
@group(1) @binding(4) var t_opacity: texture_2d<f32>;
@group(1) @binding(5) var t_tone: texture_2d<f32>;
@group(1) @binding(8) var t_detail_normal: texture_2d<f32>;
struct Bones { m: array<mat4x4<f32>, MAX_PALETTE> };
@group(1) @binding(6) var<uniform> bones: Bones;
@group(1) @binding(7) var<uniform> matflags: vec4<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) uv1: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
    @location(7) uv1: vec2<f32>,
) -> VsOut {
    var p = pos;
    var nrm = normal;
    var tan = tangent.xyz;
    let wsum = weights.x + weights.y + weights.z + weights.w;
    if (wsum > 0.001) {
        let m = weights.x * bones.m[joints.x]
              + weights.y * bones.m[joints.y]
              + weights.z * bones.m[joints.z]
              + weights.w * bones.m[joints.w];
        p = (m * vec4<f32>(pos, 1.0)).xyz;
        let r = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
        nrm = r * normal;
        tan = r * tangent.xyz;
    }
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(p, 1.0);
    out.world = p;
    out.normal = nrm;
    out.tangent = vec4<f32>(tan, tangent.w);
    out.uv = uv * u.uv_xform.xy + u.uv_xform.zw;
    out.uv1 = uv1 * u.uv_xform.xy + u.uv_xform.zw;
    out.color = color;
    return out;
}

fn s2l(c: vec3<f32>) -> vec3<f32> { return pow(c, vec3<f32>(2.2)); }

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if (u.flags.y > 0.5) { return vec4<f32>(0.55, 0.85, 0.70, 1.0); }
    var diffuse = textureSample(t_diffuse, samp, in.uv);
    if (u.map_on.x < 0.5) { diffuse = vec4<f32>(1.0, 1.0, 1.0, 1.0); } else { diffuse = diffuse * u.diffuse_mask; }
    let n_geo = normalize(in.normal);
    var n = n_geo;
    if (u.map_on.y > 0.5) {
        let t = normalize(in.tangent.xyz - n_geo * dot(n_geo, in.tangent.xyz));
        let b = cross(n_geo, t) * in.tangent.w;
        let ns = mix(vec3<f32>(0.5, 0.5, 1.0), textureSample(t_normal, samp, in.uv).xyz, u.normal_mask.xyz);
        var nm = ns * 2.0 - 1.0;
        let dnm = textureSample(t_detail_normal, samp, in.uv1).xyz * 2.0 - 1.0;
        nm = vec3<f32>(nm.xy + dnm.xy, nm.z);
        nm.z = sqrt(max(1.0 - nm.x * nm.x - nm.y * nm.y, 0.0));
        n = normalize(t * nm.x + b * nm.y + n_geo * nm.z);
    }
    if (!front) { n = -n; }

    let l = normalize(-u.light_dir.xyz);
    let up = n.y * 0.5 + 0.5;
    let amb = mix(vec3<f32>(0.30, 0.27, 0.26), vec3<f32>(0.38, 0.42, 0.48), up);
    let base = s2l(diffuse.rgb);
    let view_dir = normalize(u.eye.xyz - in.world);

    let h = normalize(l + view_dir);
    let s_map = textureSample(t_specular, samp, in.uv).r * u.specular_mask.r * f32(u.map_on.z > 0.5);
    let nvar = fwidth(n.x) + fwidth(n.y) + fwidth(n.z);
    let aa = 1.0 / (1.0 + 45.0 * nvar);
    let blinn = pow(max(dot(n, h), 0.0), max(12.0 * aa, 5.0));
    let fresnel = pow(1.0 - max(dot(n, view_dir), 0.0), 5.0);
    let spec = (blinn + 0.02 * fresnel) * s_map * 0.5 * aa;
    let nl = dot(n, l);
    let tu = clamp(1.0 - (nl * 0.5 + 0.5), 0.004, 0.996);
    var tone = textureSample(t_tone, samp, vec2<f32>(tu, 0.5)).rgb * u.tone_mask.rgb;
    if (u.map_on2.x < 0.5) { tone = vec3<f32>(clamp(nl * 0.5 + 0.5, 0.0, 1.0)); }
    let direct = s2l(tone);
    let lit = base * (amb + direct) + vec3<f32>(spec);
    let col = pow(max(lit, vec3<f32>(0.0)), vec3<f32>(1.13));

    let opac = mix(1.0, textureSample(t_opacity, samp, in.uv).g * u.opacity_mask.g, f32(u.map_on.w > 0.5));
    let dalpha = mix(1.0, diffuse.a, f32(matflags.x > 0.5));
    let mask = min(dalpha, opac);
    let cov = clamp((mask - 0.5) / max(fwidth(mask), 1e-4) + 0.5, 0.0, 1.0);
    return vec4<f32>(col, cov);
}
"#;

pub(crate) const LINE_SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
    uv_xform: vec4<f32>,
    flags: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct LnOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) hot: f32,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) hot: f32) -> LnOut {
    var out: LnOut;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.hot = hot;
    return out;
}

@fragment
fn fs_main(in: LnOut) -> @location(0) vec4<f32> {
    let col = mix(vec3<f32>(0.15, 1.0, 0.55), vec3<f32>(1.0, 0.78, 0.12), in.hot);
    return vec4<f32>(col, 1.0);
}
"#;

pub(crate) struct GpuMesh {
    pub(crate) vbuf: wgpu::Buffer,
    pub(crate) ibuf: wgpu::Buffer,
    pub(crate) index_count: u32,
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) two_sided: bool,
    pub(crate) real: Option<usize>,
    pub(crate) palette: Vec<u32>,
    pub(crate) bones_buf: wgpu::Buffer,
    pub(crate) real_bones: wgpu::BindGroup,
    /// 0 is the active model; anything else is a static extra.
    pub(crate) model_id: usize,
    /// Set only when the material has a game shader, for deferred pipeline assignment.
    pub(crate) mat_name: Option<String>,
}

/// Self-contained, so a few can be processed per frame without touching the models.
pub(crate) struct CompileJob {
    model_id: usize,
    mat: ff13::formats::model::MatTex,
}

/// A cheap-to-clone, `Send` subset of the GPU, so mesh and texture building can run on a worker.
/// Game shaders are compiled later on the main `Gpu` and are deliberately excluded.
#[derive(Clone)]
pub(crate) struct BuildCtx {
    device: std::sync::Arc<wgpu::Device>,
    queue: std::sync::Arc<wgpu::Queue>,
    tex_layout: std::sync::Arc<wgpu::BindGroupLayout>,
    sampler: std::sync::Arc<wgpu::Sampler>,
    real_bones_layout: std::sync::Arc<wgpu::BindGroupLayout>,
    defaults: std::sync::Arc<[wgpu::TextureView; 5]>,
}

impl BuildCtx {
    /// Off-thread safe.
    pub(crate) fn build_meshes(
        &self,
        model: &Model,
        skin: Option<&[Mat4]>,
        model_id: usize,
    ) -> (Vec<GpuMesh>, Vec<wgpu::TextureView>) {
        use rayon::prelude::*;
        let device = &self.device;
        let queue = &self.queue;
        // The bulk of load time, and independent per texture.
        let tex_views: Vec<wgpu::TextureView> = model
            .textures
            .par_iter()
            .map(|t| make_view(device, queue, t))
            .collect();
        let view = |slot: Option<usize>, default: usize| -> &wgpu::TextureView {
            slot.and_then(|i| tex_views.get(i))
                .unwrap_or(&self.defaults[default])
        };
        let mut meshes = Vec::new();
        for m in model.meshes.iter().filter(|m| !m.indices.is_empty()) {
            let bones_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ff13-model-bones"),
                contents: bytemuck::cast_slice(&palette_bytes(&m.palette, skin)),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let matflags = [
                if m.tex.diffuse_alpha_cutout {
                    1.0f32
                } else {
                    0.0
                },
                0.0,
                0.0,
                0.0,
            ];
            let matflags_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ff13-model-matflags"),
                contents: bytemuck::cast_slice(&matflags),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ff13-model-mesh-bg"),
                layout: &self.tex_layout,
                entries: &[
                    bind_tex(0, view(m.tex.diffuse, 0)),
                    bind_tex(1, view(m.tex.normal, 1)),
                    bind_tex(2, view(m.tex.specular, 2)),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    bind_tex(4, view(m.tex.opacity, 3)),
                    bind_tex(5, view(m.tex.tone, 4)),
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: bones_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: matflags_buf.as_entire_binding(),
                    },
                    bind_tex(8, view(m.tex.detail_normal, 1)),
                ],
            });
            let real_bones = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ff13-real-bones-bg"),
                layout: &self.real_bones_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: bones_buf.as_entire_binding(),
                }],
            });
            let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ff13-model-vbuf"),
                contents: bytemuck::cast_slice(&m.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ff13-model-ibuf"),
                contents: bytemuck::cast_slice(&m.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            meshes.push(GpuMesh {
                vbuf,
                ibuf,
                index_count: m.indices.len() as u32,
                bind_group,
                two_sided: m.tex.two_sided,
                real: None,
                palette: m.palette.clone(),
                bones_buf,
                real_bones,
                model_id,
                mat_name: m.tex.ps.is_some().then(|| m.tex.name.clone()),
            });
        }
        (meshes, tex_views)
    }

    pub(crate) fn build_lines(&self, lines: &[LineVertex]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ff13-model-skeleton"),
                contents: bytemuck::cast_slice(lines),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            })
    }
}

pub(crate) struct Target {
    pub(crate) size: (u32, u32),
    pub(crate) msaa_color: wgpu::TextureView,
    pub(crate) msaa_depth: wgpu::TextureView,
    pub(crate) resolve: wgpu::TextureView,
    pub(crate) tex_id: egui::TextureId,
}

pub(crate) struct Gpu {
    pub(crate) rs: egui_wgpu::RenderState,
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) nocull_pipeline: wgpu::RenderPipeline,
    pub(crate) wire_pipeline: Option<wgpu::RenderPipeline>,
    pub(crate) line_pipeline: wgpu::RenderPipeline,
    pub(crate) uniform_buf: wgpu::Buffer,
    pub(crate) uniform_group: wgpu::BindGroup,
    pub(crate) tex_layout: std::sync::Arc<wgpu::BindGroupLayout>,
    pub(crate) sampler: std::sync::Arc<wgpu::Sampler>,
    pub(crate) defaults: std::sync::Arc<[wgpu::TextureView; 5]>,
    pub(crate) meshes: Vec<GpuMesh>,
    pub(crate) skeleton_vbuf: Option<wgpu::Buffer>,
    pub(crate) skeleton_verts: u32,
    pub(crate) target: Option<Target>,
    pub(crate) reals: Vec<RealMat>,
    pub(crate) real_vs_buf: wgpu::Buffer,
    pub(crate) real_bones_layout: std::sync::Arc<wgpu::BindGroupLayout>,
    pub(crate) default_cube: wgpu::TextureView,
    pub(crate) default_cube_tex: wgpu::Texture,
    pub(crate) reflect_cube: wgpu::TextureView,
    pub(crate) reflect_cube_tex: wgpu::Texture,
    pub(crate) hidden: HashSet<usize>,
    /// Kept so game shaders can be compiled lazily later.
    pub(crate) model_tex_views: HashMap<usize, Vec<wgpu::TextureView>>,
    /// Reused by `repose`, so posing allocates nothing per frame.
    palette_scratch: Vec<[[f32; 4]; 4]>,
}

pub(crate) fn uniform_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ff13-model-uni"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

pub(crate) fn texture_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ff13-model-tex"),
        entries: &[
            tex_entry(0),
            tex_entry(1),
            tex_entry(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex_entry(4),
            tex_entry(5),
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            tex_entry(8),
        ],
    })
}

pub(crate) fn build_model_pipeline(
    device: &wgpu::Device,
    uni_layout: &wgpu::BindGroupLayout,
    tex_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    polygon_mode: wgpu::PolygonMode,
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ff13-model-pl"),
        bind_group_layouts: &[uni_layout, tex_layout],
        push_constant_ranges: &[],
    });
    // `vertex_attr_array`'s sequential offsets happen to land uv1 at byte 96, with no padding.
    const ATTRS: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x2, 4 => Float32x4,
        5 => Uint32x4, 6 => Float32x4, 7 => Float32x2
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ff13-model-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_main",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_main",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                // Omitting alpha keeps A2C cutout working while the resolved image stays opaque.
                write_mask: wgpu::ColorWrites::COLOR,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode,
            polygon_mode,
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
            count: MSAA,
            alpha_to_coverage_enabled: true,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

pub(crate) fn build_line_pipeline(
    device: &wgpu::Device,
    uni_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ff13-line-pl"),
        bind_group_layouts: &[uni_layout],
        push_constant_ranges: &[],
    });
    const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ff13-line-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_main",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LineVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_main",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::COLOR,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

impl Gpu {
    pub(crate) fn new(rs: &egui_wgpu::RenderState) -> Gpu {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ff13-model-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });

        let uni_layout = uniform_bind_group_layout(device);
        let tex_layout = texture_bind_group_layout(device);

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ff13-model-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ff13-model-uni-bg"),
            layout: &uni_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ff13-model-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let defaults = [
            make_view(device, &rs.queue, &flat(&[210, 210, 214, 255])),
            make_view(device, &rs.queue, &flat(&[128, 128, 255, 255])),
            make_view(device, &rs.queue, &flat(&[28, 28, 28, 255])),
            make_view(device, &rs.queue, &flat(&[255, 255, 255, 255])),
            make_view(device, &rs.queue, &default_tone_ramp()),
        ];

        let fill = wgpu::PolygonMode::Fill;
        let pipeline = build_model_pipeline(
            device,
            &uni_layout,
            &tex_layout,
            &shader,
            fill,
            Some(wgpu::Face::Back),
        );
        let nocull_pipeline =
            build_model_pipeline(device, &uni_layout, &tex_layout, &shader, fill, None);
        let wire_pipeline = device
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE)
            .then(|| {
                build_model_pipeline(
                    device,
                    &uni_layout,
                    &tex_layout,
                    &shader,
                    wgpu::PolygonMode::Line,
                    None,
                )
            });

        let real_vs_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ff13-real-vs"),
            size: std::mem::size_of::<VsU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (default_cube_tex, default_cube) =
            make_ambient_cube(device, &rs.queue, &LightRig::default());
        let (reflect_cube_tex, reflect_cube) = make_reflect_cube(device, &rs.queue, 1.0, None);
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ff13-line-shader"),
            source: wgpu::ShaderSource::Wgsl(LINE_SHADER.into()),
        });
        let line_pipeline = build_line_pipeline(device, &uni_layout, &line_shader);

        Gpu {
            rs: rs.clone(),
            pipeline,
            nocull_pipeline,
            wire_pipeline,
            line_pipeline,
            uniform_buf,
            uniform_group,
            tex_layout: std::sync::Arc::new(tex_layout),
            sampler: std::sync::Arc::new(sampler),
            defaults: std::sync::Arc::new(defaults),
            meshes: Vec::new(),
            skeleton_vbuf: None,
            skeleton_verts: 0,
            target: None,
            reals: Vec::new(),
            real_vs_buf,
            real_bones_layout: std::sync::Arc::new(real_bones_layout(device)),
            default_cube,
            default_cube_tex,
            reflect_cube,
            reflect_cube_tex,
            hidden: HashSet::new(),
            model_tex_views: HashMap::new(),
            palette_scratch: Vec::new(),
        }
    }

    /// Replaces any current scene.
    pub(crate) fn build_ctx(&self) -> BuildCtx {
        BuildCtx {
            device: self.rs.device.clone(),
            queue: self.rs.queue.clone(),
            tex_layout: self.tex_layout.clone(),
            sampler: self.sampler.clone(),
            real_bones_layout: self.real_bones_layout.clone(),
            defaults: self.defaults.clone(),
        }
    }

    /// Replaces any current scene.
    pub(crate) fn install_primary(
        &mut self,
        meshes: Vec<GpuMesh>,
        tex_views: Vec<wgpu::TextureView>,
        skeleton_vbuf: Option<wgpu::Buffer>,
        skeleton_verts: u32,
    ) {
        self.meshes = meshes;
        self.reals.clear();
        self.model_tex_views.clear();
        self.model_tex_views.insert(0, tex_views);
        self.hidden.clear();
        self.skeleton_vbuf = skeleton_vbuf;
        self.skeleton_verts = skeleton_verts;
    }

    /// Static, in native coordinates.
    pub(crate) fn install_extra(
        &mut self,
        model_id: usize,
        meshes: Vec<GpuMesh>,
        tex_views: Vec<wgpu::TextureView>,
    ) {
        self.meshes.extend(meshes);
        self.model_tex_views.insert(model_id, tex_views);
    }

    /// A render-pass skip, with no rebuild.
    pub(crate) fn set_model_hidden(&mut self, model_id: usize, hidden: bool) {
        if hidden {
            self.hidden.insert(model_id);
        } else {
            self.hidden.remove(&model_id);
        }
    }

    pub(crate) fn remove_model(&mut self, model_id: usize) {
        self.meshes.retain(|m| m.model_id != model_id);
        self.hidden.remove(&model_id);
        self.model_tex_views.remove(&model_id);
        let mut used = vec![false; self.reals.len()];
        for m in &self.meshes {
            if let Some(r) = m.real {
                used[r] = true;
            }
        }
        if used.iter().all(|&u| u) {
            return;
        }
        let mut remap = vec![None; self.reals.len()];
        let mut kept = 0usize;
        for (i, &u) in used.iter().enumerate() {
            if u {
                remap[i] = Some(kept);
                kept += 1;
            }
        }
        let mut i = 0;
        self.reals.retain(|_| {
            let keep = used[i];
            i += 1;
            keep
        });
        for m in &mut self.meshes {
            m.real = m.real.and_then(|r| remap[r]);
        }
    }

    /// The scene falls back to the built-in shader, which is what an incremental recompile wants.
    pub(crate) fn reset_reals(&mut self) {
        self.reals.clear();
        for m in &mut self.meshes {
            m.real = None;
        }
    }

    pub(crate) fn plan_compile(&self, models: &[(usize, &Model)]) -> Vec<CompileJob> {
        let mut jobs = Vec::new();
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        for &(model_id, model) in models {
            if !self.model_tex_views.contains_key(&model_id) {
                continue;
            }
            for mesh in model.meshes.iter().filter(|m| !m.indices.is_empty()) {
                if mesh.tex.ps.is_none() {
                    continue;
                }
                if seen.insert((model_id, mesh.tex.name.clone())) {
                    jobs.push(CompileJob {
                        model_id,
                        mat: mesh.tex.clone(),
                    });
                }
            }
        }
        jobs
    }

    pub(crate) fn compile_job(&mut self, job: &CompileJob, real_vs: bool, rig: &LightRig) {
        let Some(ps) = &job.mat.ps else { return };
        let rm = {
            let Some(views) = self.model_tex_views.get(&job.model_id) else {
                return;
            };
            build_real(
                &self.rs.device,
                views,
                &self.defaults,
                &self.default_cube,
                &self.reflect_cube,
                &self.sampler,
                &self.real_vs_buf,
                &self.real_bones_layout,
                &job.mat.name,
                ps,
                job.mat.vs.as_ref(),
                real_vs,
                rig,
                &job.mat.sampler_tex,
                &job.mat.color_samplers,
                MSAA,
                job.mat.cutout,
            )
        };
        if let Some(rm) = rm {
            let idx = self.reals.len();
            self.reals.push(rm);
            for m in self.meshes.iter_mut() {
                if m.model_id == job.model_id
                    && m.mat_name.as_deref() == Some(job.mat.name.as_str())
                {
                    m.real = Some(idx);
                }
            }
        }
    }

    /// `rebuild_reflection` regenerates the cube mips, so pass it only when gloss changed.
    pub(crate) fn relight(
        &self,
        rig: &LightRig,
        env_cube: Option<&std::path::Path>,
        rebuild_reflection: bool,
        anim: Option<&ff13::formats::mot::Motion>,
        frame: f32,
    ) {
        let queue = &self.rs.queue;
        write_cube_faces(queue, &self.default_cube_tex, &ambient_cube_faces(rig));
        if rebuild_reflection {
            write_reflect_cube(queue, &self.reflect_cube_tex, rig.gloss, env_cube);
        }
        self.write_material_consts(rig, anim, frame);
    }

    /// Layers the clip's material curves at `frame` over the resolved constant values.
    pub(crate) fn write_material_consts(
        &self,
        rig: &LightRig,
        anim: Option<&ff13::formats::mot::Motion>,
        frame: f32,
    ) {
        let queue = &self.rs.queue;
        for rm in &self.reals {
            let overlay = anim
                .map(|m| m.material_anims())
                .and_then(|a| a.iter().find(|a| a.material == rm.mat_name))
                .map(|a| a.overlay_at(frame));
            let consts = fill_consts(&rm.ps_constants, rm.const_count, rig, overlay.as_ref());
            let consts = consts_with_gamma(consts, rm.const_count, rig.gamma_exp);
            queue.write_buffer(&rm.const_buf, 0, bytemuck::cast_slice(&consts));
            if let (Some(buf), Some(vs)) = (&rm.vs_const_buf, &rm.vs_shader) {
                let c = fill_vs_consts(vs, rm.vs_const_count, rig, overlay.as_ref());
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&c));
            }
        }
    }

    /// Tells the viewer whether to push constants as the frame moves.
    pub(crate) fn has_material_anim(&self, anim: &ff13::formats::mot::Motion) -> bool {
        anim.material_anims()
            .iter()
            .any(|a| self.reals.iter().any(|rm| rm.mat_name == a.material))
    }

    pub(crate) fn repose(&mut self, skin: &[Mat4], lines: &[LineVertex]) {
        let queue = &self.rs.queue;
        let scratch = &mut self.palette_scratch;
        for mesh in &self.meshes {
            if mesh.model_id != 0 {
                continue;
            }
            palette_bytes_into(&mesh.palette, Some(skin), scratch);
            queue.write_buffer(&mesh.bones_buf, 0, bytemuck::cast_slice(scratch));
        }
        if let Some(vbuf) = &self.skeleton_vbuf {
            queue.write_buffer(vbuf, 0, bytemuck::cast_slice(lines));
        }
    }

    pub(crate) fn ensure_target(&mut self, size: (u32, u32)) {
        if self.target.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        let device = &self.rs.device;
        let make = |format, usage, samples| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("ff13-model-target"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let att = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa_color = make(COLOR_FORMAT, att, MSAA);
        let msaa_depth = make(DEPTH_FORMAT, att, MSAA);
        let resolve = make(COLOR_FORMAT, att | wgpu::TextureUsages::TEXTURE_BINDING, 1);
        // Rebinding the existing id avoids leaking a renderer texture per resize.
        let tex_id = match self.target.take() {
            Some(old) => {
                self.rs
                    .renderer
                    .write()
                    .update_egui_texture_from_wgpu_texture(
                        device,
                        &resolve,
                        wgpu::FilterMode::Linear,
                        old.tex_id,
                    );
                old.tex_id
            }
            None => self.rs.renderer.write().register_native_texture(
                device,
                &resolve,
                wgpu::FilterMode::Linear,
            ),
        };
        self.target = Some(Target {
            size,
            msaa_color,
            msaa_depth,
            resolve,
            tex_id,
        });
    }

    pub(crate) fn render(&self, uniforms: &Uniforms, view: Mat4, real: bool, draw_skeleton: bool) {
        let Some(target) = &self.target else { return };
        self.rs
            .queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(uniforms));
        let vsu = VsU {
            mvp: uniforms.view_proj,
            eye: uniforms.eye,
            view: view.to_cols_array_2d(),
            viewit: view.inverse().transpose().to_cols_array_2d(),
        };
        self.rs
            .queue
            .write_buffer(&self.real_vs_buf, 0, bytemuck::bytes_of(&vsu));

        let mut encoder = self
            .rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ff13-model-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ff13-model-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa_color,
                    resolve_target: Some(&target.resolve),
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
                    view: &target.msaa_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let wire = uniforms.flags[1] > 0.5;
            let force_two_sided = uniforms.flags[2] > 0.5;
            for mesh in &self.meshes {
                if self.hidden.contains(&mesh.model_id) {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vbuf.slice(..));
                pass.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                if real && let Some(ri) = mesh.real {
                    let rm = &self.reals[ri];
                    pass.set_pipeline(&rm.pipeline);
                    pass.set_bind_group(0, &rm.group0, &[]);
                    pass.set_bind_group(1, &rm.group1, &[]);
                    pass.set_bind_group(2, &mesh.real_bones, &[]);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    continue;
                }
                let pipeline = match (wire, &self.wire_pipeline) {
                    (true, Some(wp)) => wp,
                    _ if mesh.two_sided || force_two_sided => &self.nocull_pipeline,
                    _ => &self.pipeline,
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.uniform_group, &[]);
                pass.set_bind_group(1, &mesh.bind_group, &[]);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
            if draw_skeleton && let Some(vbuf) = &self.skeleton_vbuf {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.uniform_group, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.draw(0..self.skeleton_verts, 0..1);
            }
        }
        self.rs.queue.submit(std::iter::once(encoder.finish()));
    }
}

pub(crate) fn bind_tex(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(crate) fn flat(rgba: &[u8; 4]) -> TexData {
    TexData {
        width: 1,
        height: 1,
        rgba: rgba.to_vec(),
        mips: Vec::new(),
    }
}

pub(crate) fn default_tone_ramp() -> TexData {
    let warm = [1.06f32, 0.96, 0.83];
    let mut rgba = Vec::with_capacity(256 * 4);
    for u in 0..256 {
        let nl = 1.0 - 2.0 * (u as f32 / 255.0);
        let wrap = (nl + 0.3).max(0.0) / 1.3;
        let key = nl.max(0.0) * 0.62 + wrap * 0.22;
        for &w in &warm {
            let lin = (w * key).clamp(0.0, 1.0);
            rgba.push((lin.powf(1.0 / 2.2) * 255.0).round() as u8);
        }
        rgba.push(255);
    }
    TexData {
        width: 256,
        height: 1,
        rgba,
        mips: Vec::new(),
    }
}

pub(crate) fn make_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &TexData,
) -> wgpu::TextureView {
    let (w0, h0) = (tex.width.max(1), tex.height.max(1));
    // Some sequel DDS files carry a longer mip chain than their base size allows.
    let max_levels = 32 - w0.max(h0).leading_zeros();
    let mip_level_count = (1 + tex.mips.len() as u32).min(max_levels);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ff13-model-tex"),
        size: wgpu::Extent3d {
            width: w0,
            height: h0,
            depth_or_array_layers: 1,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Colour maps are sRGB-decoded in-shader, so an sRGB format would double-decode.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let upload = |level: u32, lvl: &TexData| {
        let (w, h) = (lvl.width.max(1), lvl.height.max(1));
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lvl.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    };
    upload(0, tex);
    for (i, lvl) in tex
        .mips
        .iter()
        .take(mip_level_count as usize - 1)
        .enumerate()
    {
        upload(1 + i as u32, lvl);
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct W(std::thread::Thread);
    impl Wake for W {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(W(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
