//! Renderer-agnostic model extraction from a model `.trb` and its `.imgb`.

use std::collections::HashMap;

use crate::d3d9shader::Shader;
use crate::{imgb, skl, trb::Trb, wrb};

/// Must match the renderer's bone-uniform array size; 255 fits the GL 16 KiB uniform cap.
pub const MAX_PALETTE: usize = 255;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
    pub uv1: [f32; 2],
}

pub struct TexData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The DDS's own levels, so a renderer uploads them instead of regenerating on the CPU.
    pub mips: Vec<TexData>,
}

#[derive(Default, Clone)]
pub struct MatTex {
    pub diffuse: Option<usize>,
    pub normal: Option<usize>,
    pub specular: Option<usize>,
    pub detail_normal: Option<usize>,
    pub opacity: Option<usize>,
    pub cutout: bool,
    pub diffuse_alpha_cutout: bool,
    pub two_sided: bool,
    pub tone: Option<usize>,
    pub name: String,
    pub sampler_tex: HashMap<String, usize>,
    pub color_samplers: std::collections::HashSet<String>,
    pub ps: Option<Shader>,
    pub vs: Option<Shader>,
}

pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub tex: MatTex,
    pub palette: Vec<u32>,
}

#[derive(Default)]
pub struct Model {
    pub meshes: Vec<MeshData>,
    pub textures: Vec<TexData>,
    pub skeleton: Option<skl::Skeleton>,
    pub phb: Vec<crate::phb::Phb>,
}

pub fn parse(trb_bytes: &[u8], imgb_bytes: &[u8]) -> Model {
    parse_with_packages(trb_bytes, imgb_bytes, &[])
}

/// Ids this model's materials name but its own TRB does not supply. Each is a sibling model dir
/// holding only textures, at `<id>/bin/<id>.win32.trb`.
pub fn texture_packages(trb_bytes: &[u8]) -> Vec<String> {
    let Ok(trb) = Trb::parse(trb_bytes) else {
        return Vec::new();
    };
    let local: std::collections::HashSet<String> = trb
        .resource_names()
        .iter()
        .map(|n| res_base(n).to_string())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for i in 0..trb.resource_count() {
        let Some(d) = trb.resource_data(i) else {
            continue;
        };
        if !d.starts_with(b"SEDBshd") {
            continue;
        }
        for (_, tex) in crate::sedbshd::sampler_bindings(d) {
            if local.contains(&tex) {
                continue;
            }
            // The package id is the leading lowercase/digit run of the texture name.
            let id: String = tex
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                .collect();
            if id.len() >= 3 && !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// Like [`parse`], but also draws textures from the `(trb, imgb)` pairs in `packages`.
pub fn parse_with_packages(
    trb_bytes: &[u8],
    imgb_bytes: &[u8],
    packages: &[(Vec<u8>, Vec<u8>)],
) -> Model {
    let Ok(trb) = Trb::parse(trb_bytes) else {
        return Model::default();
    };
    let names = trb.resource_names();
    let rids = trb.rid_names().map(|(r, _)| r).unwrap_or_default();

    let skeleton = trb.skeleton();
    let phb: Vec<crate::phb::Phb> = (0..trb.resource_count())
        .filter_map(|r| trb.resource_data(r))
        .filter_map(crate::phb::Phb::parse)
        .collect();
    let name_to_joint: HashMap<String, u32> = skeleton
        .as_ref()
        .map(|s| {
            s.joints
                .iter()
                .enumerate()
                .map(|(i, j)| (j.name.clone(), i as u32))
                .collect()
        })
        .unwrap_or_default();

    let mut model = Model::default();
    use rayon::prelude::*;
    let decoded: Vec<(usize, TexData)> = trb
        .texture_resources()
        .par_iter()
        .filter_map(|&res| decode_texture_data(&trb, res, imgb_bytes).map(|td| (res, td)))
        .collect();
    let mut res_to_idx: HashMap<usize, usize> = HashMap::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (res, td) in decoded {
        res_to_idx.insert(res, model.textures.len());
        if let Some(n) = names.get(res) {
            by_name.insert(res_base(n).to_string(), model.textures.len());
        }
        model.textures.push(td);
    }
    for (pkg_trb, pkg_imgb) in packages {
        let Ok(pkg) = Trb::parse(pkg_trb) else {
            continue;
        };
        let pkg_names = pkg.resource_names();
        let decoded: Vec<(usize, TexData)> = pkg
            .texture_resources()
            .par_iter()
            .filter_map(|&res| decode_texture_data(&pkg, res, pkg_imgb).map(|td| (res, td)))
            .collect();
        for (res, td) in decoded {
            let Some(n) = pkg_names.get(res) else {
                continue;
            };
            by_name
                .entry(res_base(n).to_string())
                .or_insert(model.textures.len());
            model.textures.push(td);
        }
    }

    let mut mat_cache: HashMap<String, MatTex> = HashMap::new();
    for i in 0..trb.resource_count() {
        let Some(d) = trb.resource_data(i) else {
            continue;
        };
        if !d.starts_with(b"SEDBwrb") {
            continue;
        }
        let Ok(root) = wrb::parse(d) else { continue };
        let mut mesh_chunks = Vec::new();
        collect_mesh_chunks(&root, None, &mut mesh_chunks);
        for (mc, comp) in mesh_chunks {
            let Some(sm) = mc.submesh() else { continue };
            let Some((vertices, indices)) = build_geometry(&sm, comp) else {
                continue;
            };
            let palette: Vec<u32> = mc
                .children()
                .iter()
                .filter_map(|c| c.as_envd())
                .map(|e| name_to_joint.get(&e.bone).copied().unwrap_or(u32::MAX))
                .collect();
            let tex = mesh_material(mc)
                .map(|mat| {
                    resolve_material(
                        &mat,
                        &trb,
                        &names,
                        &rids,
                        &res_to_idx,
                        &by_name,
                        &mut mat_cache,
                    )
                })
                .unwrap_or_default();
            model.meshes.push(MeshData {
                vertices,
                indices,
                tex,
                palette,
            });
        }
    }
    model.skeleton = skeleton;
    model.phb = phb;
    model
}

/// Pairs each mesh with the `COMP` box int16 positions decode through, not its own AABB.
pub fn collect_mesh_chunks<'a>(
    c: &'a wrb::Chunk,
    inherited: Option<wrb::Aabb>,
    out: &mut Vec<(&'a wrb::Chunk, Option<wrb::Aabb>)>,
) {
    let comp = c.child_comp().or(inherited);
    if c.magic() == *b"MESH" {
        out.push((c, comp));
    }
    for child in c.children() {
        collect_mesh_chunks(child, comp, out);
    }
}

pub fn build_geometry(
    sm: &wrb::Submesh,
    comp: Option<wrb::Aabb>,
) -> Option<(Vec<Vertex>, Vec<u32>)> {
    let p = &sm.positions;
    let n = p.vert_count;
    // A stride of 0 lets a malformed header claim billions of vertices backed by no bytes.
    if n == 0 || p.external || p.stride == 0 {
        return None;
    }
    let normals = sm.stream_with(2);
    let tangents = sm.stream_with(13);
    let colors = sm.stream_with(3);
    let uvs = sm.stream_with(8);
    let uvs1 = sm.stream_with(9);
    let bone_idx = sm.stream_with(15);
    let bone_wt = sm.stream_with(14);
    let mut vertices = Vec::with_capacity(n as usize);
    for i in 0..n {
        let pos = match comp {
            Some(c) => sm.position_world(i, &c),
            None => p.position_norm(i),
        }
        .unwrap_or([0.0; 3]);
        let tangent = tangents
            .and_then(|s| s.tangent(i))
            .map(|(t, sign)| [t[0], t[1], t[2], if sign { 1.0 } else { -1.0 }])
            .unwrap_or([1.0, 0.0, 0.0, 1.0]);
        let color = colors.and_then(|s| s.color_f32(i)).unwrap_or([1.0; 4]);
        let (joints, weights) = match (
            bone_idx.and_then(|s| s.bone_indices(i)),
            bone_wt.and_then(|s| s.bone_weights(i)),
        ) {
            (Some(idx), Some(w)) => {
                let cap = (MAX_PALETTE - 1) as u32;
                (
                    [
                        (idx[0] as u32).min(cap),
                        (idx[1] as u32).min(cap),
                        (idx[2] as u32).min(cap),
                        (idx[3] as u32).min(cap),
                    ],
                    w,
                )
            }
            _ => ([0; 4], [0.0; 4]),
        };
        let uv = uvs.and_then(|s| s.uv(i)).unwrap_or([0.0, 0.0]);
        vertices.push(Vertex {
            pos,
            normal: normals.and_then(|s| s.normal(i)).unwrap_or([0.0, 1.0, 0.0]),
            tangent,
            uv,
            color,
            joints,
            weights,
            uv1: uvs1.and_then(|s| s.uv_set(1, i)).unwrap_or(uv),
        });
    }
    let indices = match sm.indices.as_ref().and_then(|s| s.indices()) {
        Some(idx) => idx.iter().map(|&x| (x as u32).min(n - 1)).collect(),
        None => (0..n).collect(),
    };
    Some((vertices, indices))
}

pub fn mesh_material(mesh: &wrb::Chunk) -> Option<String> {
    let str_chunk = mesh.children().iter().find(|c| c.tag() == "STR")?;
    let s = String::from_utf8_lossy(&str_chunk.content())
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string();
    (!s.is_empty()).then_some(s)
}

pub fn resolve_material(
    mat_name: &str,
    trb: &Trb,
    names: &[String],
    rids: &[[u8; 16]],
    res_to_idx: &HashMap<usize, usize>,
    by_name: &HashMap<String, usize>,
    mat_cache: &mut HashMap<String, MatTex>,
) -> MatTex {
    if let Some(m) = mat_cache.get(mat_name) {
        return m.clone();
    }
    let mat_res =
        (0..trb.resource_count()).find(|&i| names.get(i).map(|n| res_base(n)) == Some(mat_name));
    let mdata = mat_res.and_then(|r| trb.resource_data(r));
    let mut cands: Vec<(usize, usize)> = Vec::new();
    if let Some(md) = mdata {
        for res in trb.texture_resources() {
            if let Some(rid) = rids.get(res)
                && let Some(off) = md.windows(16).position(|w| w == rid.as_slice())
            {
                cands.push((res, off));
            }
        }
        // Offset order is the shaders' `_sampler_00.._NN` order.
        cands.sort_by_key(|&(_, off)| off);
    }

    let pick = |prefixes: &[&str]| -> Option<usize> {
        prefixes.iter().find_map(|pre| {
            cands.iter().map(|&(r, _)| r).find(|&t| {
                names
                    .get(t)
                    .map(|n| tex_suffix(n))
                    .is_some_and(|s| s.starts_with(pre))
            })
        })
    };
    let lookup = |res: Option<usize>| res.and_then(|r| res_to_idx.get(&r).copied());

    let idx_of = |tex_base: &str| -> Option<usize> { by_name.get(tex_base).copied() };
    let bindings: Vec<(String, String, usize)> = mdata
        .map(crate::sedbshd::sampler_bindings)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(s, t)| idx_of(&t).map(|i| (s, t, i)))
        .collect();
    let bound = |prefixes: &[&str]| -> Option<usize> {
        prefixes.iter().find_map(|pre| {
            bindings
                .iter()
                .filter(|(s, _, _)| s.starts_with("_sampler_"))
                .find(|(_, t, _)| tex_suffix(t).starts_with(pre))
                .map(|(_, _, i)| *i)
        })
    };
    use crate::sedbshd::TextureRole;
    let pram = mdata.and_then(crate::sedbshd::pram);
    let tagged = |want: TextureRole, alpha_source: bool| -> Option<usize> {
        pram.as_ref()?
            .samplers
            .iter()
            .filter(|s| s.role() == Some(want) && (alpha_source || s.alpha_channel.is_none()))
            .find_map(|s| idx_of(&s.texture))
    };
    // The alpha-source fallback covers materials whose sole albedo is also the cutout.
    let diffuse = tagged(TextureRole::Diffuse, false)
        .or_else(|| tagged(TextureRole::Diffuse, true))
        .or_else(|| bound(&["C_", "C"]))
        .or_else(|| lookup(pick(&["C_", "C"])));
    let normal = tagged(TextureRole::Normal, true)
        .or_else(|| bound(&["N_", "N"]))
        .or_else(|| lookup(pick(&["N_", "N"])));
    let specular = tagged(TextureRole::Specular, true)
        .or_else(|| bound(&["S_", "S"]))
        .or_else(|| lookup(pick(&["S_", "S"])));
    let mut ps = mdata.and_then(crate::sedbshd::main_pixel_shader);
    let detail_normal = ps.as_ref().and_then(|ps| {
        let by_name = |name: &str| {
            bindings
                .iter()
                .find(|(s, _, _)| s.as_str() == name)
                .map(|(_, _, i)| *i)
        };
        let role = crate::d3d9shader::sampler_roles(ps)
            .iter()
            .find(|(_, r)| **r == crate::d3d9shader::Role::DetailNormal)
            .and_then(|(&reg, _)| ps.const_name(crate::d3d9shader::RegKind::Sampler, reg))
            .and_then(by_name);
        role.or_else(|| {
            let zw: std::collections::HashSet<usize> = crate::d3d9shader::detail_samplers(ps)
                .iter()
                .filter_map(|&reg| ps.const_name(crate::d3d9shader::RegKind::Sampler, reg))
                .filter_map(by_name)
                .collect();
            bindings
                .iter()
                .filter(|(_, _, i)| {
                    zw.contains(i)
                        && Some(*i) != normal
                        && Some(*i) != diffuse
                        && Some(*i) != specular
                })
                .find(|(_, t, _)| tex_suffix(t).starts_with('N'))
                .map(|(_, _, i)| *i)
        })
    });

    let alpha_source = match &pram {
        Some(p) => p
            .samplers
            .iter()
            .find_map(|s| s.alpha_channel.map(|c| (idx_of(&s.texture), c))),
        None => mdata.and_then(|md| {
            cands.iter().copied().find_map(|(res, off)| {
                crate::sedbshd::binding_marker(md, off)
                    .filter(|&b| b != 0xff)
                    .map(|b1| (lookup(Some(res)), b1))
            })
        }),
    };
    let diffuse_alpha_cutout = matches!(alpha_source, Some((tex, 3)) if tex == diffuse);
    let opacity = alpha_source
        .map(|(tex, _)| tex)
        .filter(|_| !diffuse_alpha_cutout)
        .flatten();
    let two_sided = mdata
        .map(crate::sedbshd::material_two_sided)
        .unwrap_or(false);
    let cutout = alpha_source.is_some();
    let tone = bindings
        .iter()
        .find(|(s, _, _)| s == "lightToneMap")
        .map(|(_, _, i)| *i)
        .or_else(|| lookup(pick(&["T_"])));
    let mut sampler_tex: HashMap<String, usize> = HashMap::new();
    let mut color_samplers: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (samp, tex_base, idx) in &bindings {
        if matches!(tex_suffix(tex_base).chars().next(), Some('C') | Some('F')) {
            color_samplers.insert(samp.clone());
        }
        sampler_tex.insert(samp.clone(), *idx);
    }
    let mut vs = mdata.and_then(crate::sedbshd::main_vertex_shader);
    if let Some(p) = &pram {
        for shader in ps.iter_mut().chain(vs.iter_mut()) {
            crate::sedbshd::apply_parameters(shader, &p.parameters);
        }
    }
    let m = MatTex {
        diffuse,
        normal,
        specular,
        detail_normal,
        opacity,
        cutout,
        diffuse_alpha_cutout,
        two_sided,
        tone,
        name: mat_name.to_string(),
        sampler_tex,
        color_samplers,
        ps,
        vs,
    };
    mat_cache.insert(mat_name.to_string(), m.clone());
    m
}

pub fn decode_texture_data(trb: &Trb, res: usize, imgb: &[u8]) -> Option<TexData> {
    let header = trb.resource_data(res)?;
    let (tex, _) = imgb::first_and_count(header, imgb).ok()?;
    dds_to_texdata(&tex.dds)
}

pub fn res_base(name: &str) -> &str {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    base.strip_suffix(".win32").unwrap_or(base)
}

pub fn tex_suffix(name: &str) -> String {
    res_base(name)
        .trim_start_matches(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        .to_string()
}

pub fn dds_to_texdata(dds: &[u8]) -> Option<TexData> {
    let parsed = ddsfile::Dds::read(std::io::Cursor::new(dds)).ok()?;
    let img = image_dds::image_from_dds(&parsed, 0).ok()?;
    let (width, height) = (img.width(), img.height());
    let rgba = img.into_raw();

    let mut mips = Vec::new();
    let mut level = 1;
    while let Ok(m) = image_dds::image_from_dds(&parsed, level) {
        mips.push(TexData {
            width: m.width(),
            height: m.height(),
            rgba: m.into_raw(),
            mips: Vec::new(),
        });
        level += 1;
    }

    Some(TexData {
        width,
        height,
        rgba,
        mips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trb::Trb;

    type Materials = (Vec<String>, Vec<(String, MatTex)>);

    /// Texture indices stay as resource indices, so the results read back as names.
    fn materials(path: &str) -> Option<Materials> {
        let bytes = std::fs::read(path).ok()?;
        let trb = Trb::parse(&bytes).ok()?;
        let names = trb.resource_names();
        let rids = trb.rid_names().map(|(r, _)| r).unwrap_or_default();
        let res_to_idx: HashMap<usize, usize> = (0..trb.resource_count()).map(|r| (r, r)).collect();
        let mut cache = HashMap::new();
        let mut out = Vec::new();
        for r in 0..trb.resource_count() {
            let Some(d) = trb.resource_data(r) else {
                continue;
            };
            if !d.starts_with(b"SEDBshd") {
                continue;
            }
            let name = res_base(names.get(r).map(|n| n.as_str()).unwrap_or("")).to_string();
            let by_name: HashMap<String, usize> = (0..trb.resource_count())
                .filter_map(|r| names.get(r).map(|n| (res_base(n).to_string(), r)))
                .collect();
            let m = resolve_material(
                &name,
                &trb,
                &names,
                &rids,
                &res_to_idx,
                &by_name,
                &mut cache,
            );
            out.push((name, m));
        }
        Some((names, out))
    }

    #[test]
    fn shipped_material_roles_follow_the_engine_semantics() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let expect = |model: &str, mat: &str, want: [Option<&str>; 4], cutout: (bool, bool)| {
            let Some((names, mats)) =
                materials(&format!("{dir}/chr/pc/{model}/bin/{model}.win32.trb"))
            else {
                return;
            };
            let m = &mats
                .iter()
                .find(|(n, _)| n == mat)
                .unwrap_or_else(|| panic!("{mat} missing"))
                .1;
            let name = |i: Option<usize>| {
                i.and_then(|r| names.get(r))
                    .map(|n| res_base(n).to_string())
            };
            let got = [m.diffuse, m.normal, m.specular, m.opacity].map(name);
            let want = want.map(|w| w.map(str::to_string));
            assert_eq!(got, want, "{mat} roles");
            assert_eq!((m.cutout, m.diffuse_alpha_cutout), cutout, "{mat} cutout");
        };
        expect(
            "c001",
            "c001_3skin",
            [Some("c001C_03"), Some("c001N_03"), Some("c001S_03"), None],
            (false, false),
        );
        expect(
            "c592",
            "c592_skin2",
            [Some("c592C_01"), Some("c592N_01"), Some("c592K_01"), None],
            (false, false),
        );
        expect(
            "c002",
            "c002_4ac_d",
            [Some("c002C_04"), Some("c002N_04"), None, Some("c002A_04")],
            (true, false),
        );
        expect(
            "c003",
            "c003_4hair",
            [Some("c003C_04"), None, Some("c003G_04"), None],
            (true, true),
        );
    }

    #[test]
    fn counts_the_name_prefix_fallbacks() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let (mut total, mut no_pram) = (0u32, 0u32);
        let (mut fallback, mut filled) = ([0u32; 3], [0u32; 3]);
        let mut stack = vec![std::path::PathBuf::from(format!("{dir}/chr/pc"))];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let Some((names, mats)) = materials(p.to_str().unwrap()) else {
                    continue;
                };
                let bytes = std::fs::read(&p).unwrap();
                let trb = Trb::parse(&bytes).unwrap();
                for (name, m) in &mats {
                    total += 1;
                    let res = (0..trb.resource_count())
                        .find(|&i| names.get(i).map(|n| res_base(n)) == Some(name.as_str()));
                    let table = res
                        .and_then(|r| trb.resource_data(r))
                        .and_then(crate::sedbshd::pram);
                    let Some(table) = table else {
                        no_pram += 1;
                        continue;
                    };
                    let roles = [
                        crate::sedbshd::TextureRole::Diffuse,
                        crate::sedbshd::TextureRole::Normal,
                        crate::sedbshd::TextureRole::Specular,
                    ];
                    for (slot, (role, tex)) in roles
                        .iter()
                        .zip([m.diffuse, m.normal, m.specular])
                        .enumerate()
                    {
                        if tex.is_none() {
                            continue;
                        }
                        filled[slot] += 1;
                        if !table.samplers.iter().any(|s| s.role() == Some(*role)) {
                            fallback[slot] += 1;
                        }
                    }
                }
            }
        }
        eprintln!(
            "{total} materials, {no_pram} without PRAM; slots filled {filled:?}, of those {fallback:?} by name prefix"
        );
        assert_eq!((total, no_pram), (227, 0));
        assert_eq!(filled, [125, 114, 100]);
        assert_eq!(fallback, [4, 4, 29]);
    }

    fn trb_paths(dir: &str) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("trb") {
                    out.push(p);
                }
            }
        }
        out
    }

    // All-zero UVs sample one corner of the atlas, which the sequel shaders square to black.
    #[test]
    fn texcoords_decode_from_whichever_stream_declares_them() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (mut meshes, mut with_uv) = (0u64, 0u64);
        let mut degenerate: Vec<String> = Vec::new();
        for p in trb_paths(&dir) {
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let Ok(trb) = Trb::parse(&bytes) else {
                continue;
            };
            for i in 0..trb.resource_count() {
                let Some(d) = trb.resource_data(i) else {
                    continue;
                };
                if !d.starts_with(b"SEDBwrb") {
                    continue;
                }
                let Ok(root) = wrb::parse(d) else { continue };
                let mut mesh_chunks = Vec::new();
                collect_mesh_chunks(&root, None, &mut mesh_chunks);
                for (mc, comp) in mesh_chunks {
                    let Some(sm) = mc.submesh() else { continue };
                    let Some((verts, _)) = build_geometry(&sm, comp) else {
                        continue;
                    };
                    meshes += 1;
                    if sm.stream_with(8).is_none() || verts.is_empty() {
                        continue;
                    }
                    with_uv += 1;
                    if verts.iter().all(|v| v.uv == [0.0, 0.0]) && degenerate.len() < 10 {
                        degenerate.push(p.display().to_string());
                    }
                }
            }
        }
        assert!(meshes > 0, "no meshes found under {dir}");
        assert!(with_uv > 0, "no mesh declared texcoords under {dir}");
        assert!(
            degenerate.is_empty(),
            "meshes with a texcoord declaration decoded all-zero UVs: {degenerate:?}"
        );
        eprintln!("{with_uv}/{meshes} meshes declare texcoords, all decode non-zero");
    }
}
