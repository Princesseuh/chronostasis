//! `SEDBshd` material resource: the `PRAM` sampler and parameter tables.

use crate::d3d9shader::{self, Shader};

fn cstr(d: &[u8], off: usize) -> String {
    if off >= d.len() {
        return String::new();
    }
    let end = d[off..]
        .iter()
        .position(|&b| b == 0)
        .map(|e| off + e)
        .unwrap_or(d.len());
    String::from_utf8_lossy(&d[off..end]).into_owned()
}

fn le32(d: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(off..off + 4)?.try_into().ok()?))
}

fn be32(d: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(off..off + 4)?.try_into().ok()?))
}

/// The role a bound texture serves, read from the sampler entry's semantic byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureRole {
    Diffuse,
    Normal,
    Specular,
    CubeMap,
    /// Bound by shader-constant name rather than by role: tone ramps, detail/multi maps, fuzz maps.
    Generic,
}

#[derive(Clone, Debug)]
pub struct Sampler {
    /// e.g. `_sampler_00`, `lightToneMap`, `ambientCubeMap`.
    pub name: String,
    /// Suffix removed. Empty when the texture is scene-supplied rather than named here.
    pub texture: String,
    /// Bit 5 marks a usable value, bit 6 an alpha source, bit 7 a cube map, low nibble the role.
    pub semantic: u8,
    /// 0 = red, 1 = green, 2 = blue, 3 = this texture's own alpha.
    pub alpha_channel: Option<u8>,
}

impl Sampler {
    /// `None` when the semantic byte has bit 5 clear and holds no readable role.
    ///
    /// The low nibble's odd values are diffuse-class; it is the alpha-source bit that makes them
    /// masks, which is what keeps hair `G` atlases out of the albedo slot.
    pub fn role(&self) -> Option<TextureRole> {
        if self.semantic & 0x20 == 0 {
            return None;
        }
        if self.semantic & 0x80 != 0 {
            return Some(TextureRole::CubeMap);
        }
        Some(match self.semantic & 0x0f {
            0x0 => TextureRole::Generic,
            0x1 | 0x3 | 0x5 | 0x7 => TextureRole::Diffuse,
            0x2 | 0x4 | 0x6 => TextureRole::Specular,
            _ => TextureRole::Normal,
        })
    }
}

/// This material's authored value for a shader constant, often not the constant's CTAB default.
#[derive(Clone, Debug)]
pub struct Parameter {
    pub name: String,
    /// Targets the pixel shader; otherwise the vertex shader.
    pub pixel_shader: bool,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Pram {
    /// Bit 3 looks like a backface-cull flag, but [`material_two_sided`] is the one that matches
    /// what the game draws.
    pub render_state: u8,
    pub samplers: Vec<Sampler>,
    pub parameters: Vec<Parameter>,
}

/// Walks the `SHD` container's sub-chunk chain rather than scanning for the tag.
fn pram_chunk(material: &[u8]) -> Option<usize> {
    if !material.starts_with(b"SEDBshd") {
        return None;
    }
    let count = be32(material, 48 + 16)?;
    let mut off = 48 + 32;
    for _ in 0..count.min(64) {
        if material.get(off..off + 16)?.starts_with(b"PRAM") {
            return Some(off);
        }
        let step = match be32(material, off + 12)? {
            0 => 16 + be32(material, off + 8)? as usize,
            next => next as usize,
        };
        off = off.checked_add(step.max(16))?;
    }
    None
}

/// `None` when the resource is not a SEDBshd, or its chunk chain reaches no readable `PRAM`.
pub fn pram(material: &[u8]) -> Option<Pram> {
    let content = pram_chunk(material)? + 16;
    let param_count = le32(material, content + 8)? as usize;
    let param_data = le32(material, content + 12)? as usize;
    let param_names = le32(material, content + 16)? as usize;
    let sampler_count = le32(material, content + 20)? as usize;
    let sampler_data = le32(material, content + 24)? as usize;
    let sampler_names = le32(material, content + 28)? as usize;
    if param_count > 1024 || sampler_count > 64 {
        return None;
    }
    let entry = |table: usize, i: usize| -> Option<usize> {
        content.checked_add(le32(material, content + table + i * 4)? as usize)
    };
    let mut samplers = Vec::with_capacity(sampler_count);
    for i in 0..sampler_count {
        let rec = entry(sampler_data, i)?;
        let name_len = *material.get(rec + 2)? as usize;
        let semantic = *material.get(rec + 3)?;
        let channel = *material.get(rec + 14)?;
        let section = material.get(rec + 32..(rec + 32 + name_len).min(material.len()))?;
        let texture = cstr(section, 0);
        samplers.push(Sampler {
            name: cstr(material, entry(sampler_names, i)?),
            texture: texture.strip_suffix(".dds").unwrap_or(&texture).to_string(),
            semantic,
            alpha_channel: (semantic & 0x40 != 0 && channel <= 3).then_some(channel),
        });
    }
    let mut parameters = Vec::with_capacity(param_count);
    for i in 0..param_count {
        let rec = entry(param_data, i)?;
        let flags = u16::from_le_bytes([*material.get(rec)?, *material.get(rec + 1)?]);
        let count = u16::from_le_bytes([*material.get(rec + 2)?, *material.get(rec + 3)?]).min(4);
        let mut values = Vec::with_capacity(count as usize);
        for k in 0..count as usize {
            values.push(f32::from_le_bytes(
                material
                    .get(rec + 16 + k * 4..rec + 20 + k * 4)?
                    .try_into()
                    .ok()?,
            ));
        }
        parameters.push(Parameter {
            name: cstr(material, entry(param_names, i)?),
            pixel_shader: flags & 1 == 1,
            values,
        });
    }
    Some(Pram {
        render_state: *material.get(content)?,
        samplers,
        parameters,
    })
}

/// Replaces each constant's compiled-in CTAB default with this material's authored value.
pub fn apply_parameters(shader: &mut Shader, parameters: &[Parameter]) {
    for p in parameters.iter().filter(|p| p.pixel_shader == shader.pixel) {
        for c in shader.constants.iter_mut() {
            if c.reg_set != 2 || c.reg_count != 1 || c.name != p.name {
                continue;
            }
            let d = c.default.get_or_insert_with(|| vec![0.0; 4]);
            d.resize(d.len().max(p.values.len()), 0.0);
            d[..p.values.len()].copy_from_slice(&p.values);
        }
    }
}

/// `(sampler name, bound texture name)` in binding order. Scene-supplied samplers map to
/// placeholder names like `"DrawEnv03Texture"`.
pub fn sampler_bindings(material: &[u8]) -> Vec<(String, String)> {
    match pram(material) {
        Some(p) => p
            .samplers
            .into_iter()
            .filter(|s| !s.texture.is_empty())
            .map(|s| (s.name, s.texture))
            .collect(),
        None => scanned_sampler_bindings(material),
    }
}

/// Byte-scan fallback for material data with no readable `PRAM`. Returns empty when the texture and
/// sampler-name counts disagree, so callers fall back rather than mis-bind.
pub fn scanned_sampler_bindings(material: &[u8]) -> Vec<(String, String)> {
    let mut tex_names = Vec::new();
    let mut o = 0;
    while o + 17 <= material.len() {
        let entry = material[o..o + 4] == [0x01, 0x01, 0xf8, 0x01]
            && material[o + 5..o + 8] == [0; 3]
            && material[o + 12..o + 16] == [0; 4]
            && material[o + 16].is_ascii_alphabetic();
        if entry {
            let name = cstr(material, o + 16);
            tex_names.push(name.strip_suffix(".dds").unwrap_or(&name).to_string());
            o += 16;
        } else {
            o += 1;
        }
    }
    let mut samp_names = Vec::new();
    if let Some(start) = material.windows(4).rposition(|w| w == b"brt\0") {
        let mut p = start + 4;
        while p < material.len() {
            if material[p] == 0 {
                p += 1;
                continue;
            }
            let s = cstr(material, p);
            p += s.len() + 1;
            if s.len() >= 2 {
                samp_names.push(s);
            }
        }
    }
    if tex_names.is_empty() || tex_names.len() != samp_names.len() {
        return Vec::new();
    }
    samp_names.into_iter().zip(tex_names).collect()
}

/// Read from the `PCAP` render-state chunk, not the [`Pram::render_state`] bit.
pub fn material_two_sided(mdata: &[u8]) -> bool {
    mdata
        .windows(4)
        .rposition(|w| w == b"PCAP")
        .and_then(|o| mdata.get(o + 36))
        .is_some_and(|&b| b & 0x40 != 0)
}

/// Byte-scan equivalent of [`Sampler::alpha_channel`]: `0xff` for ordinary textures, else the
/// source channel `0`-`3` of the at most one texture routed to the shader's alpha output.
pub fn binding_marker(mdata: &[u8], rid_off: usize) -> Option<u8> {
    binding_byte(mdata, rid_off, 2)
}

/// `back`=2 is the alpha-channel byte; `back`=3 is `0xff`, or the scene slot for engine textures.
pub fn binding_byte(mdata: &[u8], rid_off: usize, back: usize) -> Option<u8> {
    let start = rid_off.saturating_sub(80);
    let anchor = mdata[start..rid_off]
        .windows(4)
        .rposition(|w| w == [0x01, 0x01, 0xf8, 0x01])?
        + start;
    mdata.get(anchor.checked_sub(back)?).copied()
}

/// The sampler-binding pixel shader with the most CTAB constants, which is the full-quality pass.
pub fn main_pixel_shader(material: &[u8]) -> Option<Shader> {
    d3d9shader::find_shaders(material)
        .into_iter()
        .filter_map(|blob| d3d9shader::decode(blob).ok())
        .filter(|sh| sh.pixel && sh.constants.iter().any(|c| c.reg_set == 3))
        .max_by_key(|sh| sh.constants.len())
}

/// The game picks one variant per quality/LOD at runtime.
pub fn pixel_shader_variants(material: &[u8]) -> Vec<Vec<u8>> {
    d3d9shader::find_shaders(material)
        .into_iter()
        .filter(|blob| d3d9shader::decode(blob).is_ok_and(|sh| sh.pixel))
        .map(|blob| blob.to_vec())
        .collect()
}

/// The variant with the most instructions, which is the full-feature one.
pub fn main_vertex_shader(material: &[u8]) -> Option<Shader> {
    d3d9shader::find_shaders(material)
        .into_iter()
        .filter_map(|blob| d3d9shader::decode(blob).ok())
        .filter(|sh| !sh.pixel)
        .max_by_key(|sh| sh.insts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(out: &mut Vec<u8>, flag: u8, hash: u32, name: &str) {
        out.extend_from_slice(&[0x01, 0x01, 0xf8, 0x01]);
        out.extend_from_slice(&[flag, 0, 0, 0]);
        out.extend_from_slice(&hash.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(format!("{name}.dds\0").as_bytes());
        out.extend_from_slice(b"file001\0");
        out.extend_from_slice(b"brt\0");
    }

    #[test]
    fn zips_entries_with_trailing_sampler_names() {
        let mut m = Vec::new();
        entry(&mut m, 0, 0xdeadbeef, "c001C_04");
        entry(&mut m, 1, 0x12345678, "c001N_04");
        m.extend_from_slice(b"\0\0_sampler_02\0_sampler_00\0");

        let b = sampler_bindings(&m);
        assert_eq!(
            b,
            vec![
                ("_sampler_02".to_string(), "c001C_04".to_string()),
                ("_sampler_00".to_string(), "c001N_04".to_string()),
            ]
        );
    }

    #[test]
    fn mismatched_counts_return_empty() {
        let mut m = Vec::new();
        entry(&mut m, 0, 1, "c001C_04");
        m.extend_from_slice(b"\0\0_sampler_00\0_sampler_01\0");
        assert!(sampler_bindings(&m).is_empty());
    }

    /// The `PRAM` sits behind a chunk the walker has to step over.
    fn synth() -> Vec<u8> {
        let mut m = vec![0u8; 48];
        m[..7].copy_from_slice(b"SEDBshd");
        let chunk = |m: &mut Vec<u8>, cc: &[u8; 4], size: u32, next: u32| {
            m.extend_from_slice(cc);
            m.extend_from_slice(&0u32.to_be_bytes());
            m.extend_from_slice(&size.to_be_bytes());
            m.extend_from_slice(&next.to_be_bytes());
        };
        chunk(&mut m, b"SHD\0", 0, 0);
        m.extend_from_slice(&2u32.to_be_bytes());
        m.extend_from_slice(&[0; 12]);
        chunk(&mut m, b"FILE", 16, 32);
        m.extend_from_slice(&[0xaa; 16]);
        chunk(&mut m, b"PRAM", 0, 0);

        let mut c: Vec<u8> = vec![0x18, 0, 5, 0, 0, 0, 0, 0];
        for v in [1u32, 32, 36, 2, 40, 48] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        let (params, samplers) = (56u32, 88u32);
        c.extend_from_slice(&params.to_le_bytes());
        c.extend_from_slice(&184u32.to_le_bytes());
        c.extend_from_slice(&samplers.to_le_bytes());
        c.extend_from_slice(&(samplers + 48).to_le_bytes());
        c.extend_from_slice(&194u32.to_le_bytes());
        c.extend_from_slice(&206u32.to_le_bytes());

        c.extend_from_slice(&1u16.to_le_bytes());
        c.extend_from_slice(&1u16.to_le_bytes());
        c.extend_from_slice(&[0; 12]);
        for v in [6.0f32, 0.0, 0.0, 0.0] {
            c.extend_from_slice(&v.to_le_bytes());
        }

        let mut sampler = |semantic: u8, channel: u8, texture: &str| {
            c.extend_from_slice(&[2, 1, 16, semantic]);
            c.extend_from_slice(&[0; 8]);
            c.extend_from_slice(&[0, 0xff, channel, 0]);
            c.extend_from_slice(&[0; 16]);
            let mut name = texture.as_bytes().to_vec();
            name.resize(16, 0);
            c.extend_from_slice(&name);
        };
        sampler(0x21, 0xff, "c001C_01.dds");
        sampler(0x61, 0x01, "c001A_01.dds");
        c.extend_from_slice(b"shininess\0_sampler_00\0_sampler_01\0");

        m.extend_from_slice(&c);
        m
    }

    #[test]
    fn parses_the_pram_table() {
        let p = pram(&synth()).expect("PRAM");
        assert_eq!(p.render_state, 0x18);
        assert_eq!(p.parameters.len(), 1);
        assert_eq!(p.parameters[0].name, "shininess");
        assert!(p.parameters[0].pixel_shader);
        assert_eq!(p.parameters[0].values, vec![6.0]);
        assert_eq!(p.samplers.len(), 2);
        assert_eq!(p.samplers[0].name, "_sampler_00");
        assert_eq!(p.samplers[0].texture, "c001C_01");
        assert_eq!(p.samplers[0].role(), Some(TextureRole::Diffuse));
        assert_eq!(p.samplers[0].alpha_channel, None);
        assert_eq!(p.samplers[1].texture, "c001A_01");
        assert_eq!(p.samplers[1].role(), Some(TextureRole::Diffuse));
        assert_eq!(p.samplers[1].alpha_channel, Some(1));
        assert_eq!(
            sampler_bindings(&synth()),
            vec![
                ("_sampler_00".to_string(), "c001C_01".to_string()),
                ("_sampler_01".to_string(), "c001A_01".to_string()),
            ]
        );
    }

    #[test]
    fn truncation_does_not_panic() {
        let full = synth();
        for cut in 0..full.len() {
            let _ = pram(&full[..cut]);
        }
    }

    #[test]
    fn semantics_map_to_the_reference_tools_labels() {
        let role = |semantic| {
            Sampler {
                name: String::new(),
                texture: String::new(),
                semantic,
                alpha_channel: None,
            }
            .role()
        };
        for (semantic, want) in [
            (0x20, TextureRole::Generic),
            (0x21, TextureRole::Diffuse),
            (0x22, TextureRole::Specular),
            (0x24, TextureRole::Specular),
            (0x26, TextureRole::Specular),
            (0x28, TextureRole::Normal),
            (0x2a, TextureRole::Normal),
            (0x2c, TextureRole::Normal),
            (0x2e, TextureRole::Normal),
            (0x69, TextureRole::Normal),
            (0xa0, TextureRole::CubeMap),
        ] {
            assert_eq!(role(semantic), Some(want), "semantic {semantic:#04x}");
        }
        assert_eq!(role(0x01), None);
    }

    #[test]
    fn parameters_overwrite_ctab_defaults_without_touching_the_tail() {
        let mut shader = Shader {
            pixel: true,
            major: 3,
            minor: 0,
            insts: Vec::new(),
            constants: vec![crate::d3d9shader::Constant {
                name: "specularColor".to_string(),
                reg_set: 2,
                reg_index: 4,
                reg_count: 1,
                default: Some(vec![1.0, 1.0, 1.0, 0.5]),
            }],
        };
        let params = vec![
            Parameter {
                name: "specularColor".into(),
                pixel_shader: true,
                values: vec![0.0, 0.2, 0.4],
            },
            Parameter {
                name: "specularColor".into(),
                pixel_shader: false,
                values: vec![9.0],
            },
        ];
        apply_parameters(&mut shader, &params);
        assert_eq!(shader.constants[0].default, Some(vec![0.0, 0.2, 0.4, 0.5]));
    }

    #[test]
    fn shipped_materials_carry_a_readable_pram() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let (mut materials, mut samplers, mut scanned, mut no_role) = (0u64, 0u64, 0u64, 0u64);
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
                let bytes = std::fs::read(&p).unwrap();
                let Ok(trb) = crate::trb::Trb::parse(&bytes) else {
                    continue;
                };
                for r in 0..trb.resource_count() {
                    let Some(md) = trb.resource_data(r) else {
                        continue;
                    };
                    if !md.starts_with(b"SEDBshd") {
                        continue;
                    }
                    materials += 1;
                    let table = pram(md).unwrap_or_else(|| panic!("no PRAM in {}", p.display()));
                    let alpha = table
                        .samplers
                        .iter()
                        .filter(|s| s.alpha_channel.is_some())
                        .count();
                    assert!(alpha <= 1, "{} binds {alpha} alpha sources", p.display());
                    for s in &table.samplers {
                        samplers += 1;
                        if s.role().is_none() {
                            no_role += 1;
                        }
                        assert!(!s.name.is_empty(), "unnamed sampler in {}", p.display());
                        assert_eq!(
                            s.role() == Some(TextureRole::CubeMap),
                            s.texture.starts_with("DrawEnv"),
                            "{} tags {} as {:?}",
                            p.display(),
                            s.texture,
                            s.role()
                        );
                    }
                    let scan = scanned_sampler_bindings(md);
                    if scan.is_empty() {
                        continue;
                    }
                    scanned += 1;
                    let named: Vec<_> = table
                        .samplers
                        .iter()
                        .map(|s| (s.name.clone(), s.texture.clone()))
                        .collect();
                    assert_eq!(scan, named, "scan disagrees with PRAM in {}", p.display());
                }
            }
        }
        eprintln!(
            "{materials} materials, {samplers} samplers, {scanned} also readable by byte-scan"
        );
        assert_eq!((materials, samplers, no_role), (227, 960, 0));
    }

    #[test]
    fn shipped_parameters_carry_per_material_values() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/chr/pc/c001/bin/c001.win32.trb");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let trb = crate::trb::Trb::parse(&bytes).unwrap();
        let (mut matched, mut differing) = (0u64, 0u64);
        for r in 0..trb.resource_count() {
            let Some(md) = trb.resource_data(r) else {
                continue;
            };
            if !md.starts_with(b"SEDBshd") {
                continue;
            }
            let table = pram(md).unwrap();
            let Some(ps) = main_pixel_shader(md) else {
                continue;
            };
            for param in table.parameters.iter().filter(|p| p.pixel_shader) {
                let Some(c) = ps
                    .constants
                    .iter()
                    .find(|c| c.reg_set == 2 && c.name == param.name)
                else {
                    // The only shipped parameter naming a bool constant rather than a float one.
                    assert_eq!(param.name, "EnableShadowFlag");
                    continue;
                };
                matched += 1;
                let baked = c.default.clone().unwrap_or_default();
                if (0..param.values.len()).any(|i| baked.get(i) != Some(&param.values[i])) {
                    differing += 1;
                }
            }
        }
        eprintln!("c001: {matched} parameters resolved, {differing} differ from the CTAB default");
        assert_eq!((matched, differing), (99, 15));
    }
}
