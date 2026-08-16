//! Direct3D9 shader-model-3 bytecode decoder for the shaders `SEDBshd` materials embed.

use crate::{FormatError, Result};

pub const VS_3_0: u32 = 0xFFFE_0300;
pub const PS_3_0: u32 = 0xFFFF_0300;
const END_TOKEN: u32 = 0x0000_FFFF;
const COMMENT_OP: u16 = 0xFFFE;
const END_OP: u16 = 0xFFFF;
const OP_DEF: u16 = 81;
const OP_DEFI: u16 = 48;
const OP_DCL: u16 = 31;

/// The D3DSIO_* opcodes the transpiler handles.
pub mod op {
    pub const NOP: u16 = 0;
    pub const MOV: u16 = 1;
    pub const ADD: u16 = 2;
    pub const SUB: u16 = 3;
    pub const MAD: u16 = 4;
    pub const MUL: u16 = 5;
    pub const RCP: u16 = 6;
    pub const RSQ: u16 = 7;
    pub const DP3: u16 = 8;
    pub const DP4: u16 = 9;
    pub const MIN: u16 = 10;
    pub const MAX: u16 = 11;
    pub const SLT: u16 = 12;
    pub const SGE: u16 = 13;
    pub const EXP: u16 = 14;
    pub const LOG: u16 = 15;
    pub const LRP: u16 = 18;
    pub const FRC: u16 = 19;
    pub const DCL: u16 = 31;
    pub const POW: u16 = 32;
    pub const ABS: u16 = 35;
    pub const NRM: u16 = 36;
    pub const SINCOS: u16 = 37;
    pub const LOOP: u16 = 27;
    pub const ENDLOOP: u16 = 29;
    pub const REP: u16 = 38;
    pub const ENDREP: u16 = 39;
    pub const IF: u16 = 40;
    pub const IFC: u16 = 41;
    pub const ELSE: u16 = 42;
    pub const ENDIF: u16 = 43;
    pub const BREAK: u16 = 44;
    pub const BREAKC: u16 = 45;
    pub const MOVA: u16 = 46;
    pub const TEXKILL: u16 = 65;
    pub const TEXLD: u16 = 66;
    pub const DEF: u16 = 81;
    pub const DEFI: u16 = 48;
    pub const CMP: u16 = 88;
    pub const DP2ADD: u16 = 90;
}

/// D3DSHADER_PARAM_REGISTER_TYPE.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegKind {
    Temp,      // r
    Input,     // v
    Const,     // c
    Texture,   // t (also ADDR in vs)
    RastOut,   // oPos/oFog/oPts
    AttrOut,   // oD
    Output,    // o (oT in <3.0)
    ConstInt,  // i
    ColorOut,  // oC
    DepthOut,  // oDepth
    Sampler,   // s
    ConstBool, // b
    Loop,      // aL
    Misc,      // vPos / vFace
    Label,
    Predicate, // p
    Other(u8),
}

impl RegKind {
    fn from_code(c: u8) -> RegKind {
        use RegKind::*;
        match c {
            0 => Temp,
            1 => Input,
            2 => Const,
            3 => Texture,
            4 => RastOut,
            5 => AttrOut,
            6 => Output,
            7 => ConstInt,
            8 => ColorOut,
            9 => DepthOut,
            10 => Sampler,
            14 => ConstBool,
            15 => Loop,
            17 => Misc,
            18 => Label,
            19 => Predicate,
            other => Other(other),
        }
    }
    pub fn sigil(self) -> &'static str {
        use RegKind::*;
        match self {
            Temp => "r",
            Input => "v",
            Const => "c",
            Texture => "t",
            RastOut => "oRast",
            AttrOut => "oD",
            Output => "o",
            ConstInt => "i",
            ColorOut => "oC",
            DepthOut => "oDepth",
            Sampler => "s",
            ConstBool => "b",
            Loop => "aL",
            Misc => "vMisc",
            Label => "l",
            Predicate => "p",
            Other(_) => "?",
        }
    }
}

/// The D3DSPSM_* modifiers that occur in compiled shaders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SrcMod {
    None,
    Neg,
    Bias,
    BiasNeg,
    /// `_bx2`, i.e. `2*x-1`.
    Sign,
    SignNeg,
    /// `1-x`.
    Comp,
    Abs,
    AbsNeg,
    Other(u8),
}

impl SrcMod {
    fn from_code(c: u8) -> SrcMod {
        use SrcMod::*;
        match c {
            0 => None,
            1 => Neg,
            2 => Bias,
            3 => BiasNeg,
            4 => Sign,
            5 => SignNeg,
            6 => Comp,
            11 => Abs,
            12 => AbsNeg,
            other => Other(other),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Operand {
    pub kind: RegKind,
    pub index: u16,
    /// Identity on a destination.
    pub swizzle: [u8; 4],
    /// `x=1,y=2,z=4,w=8`; always `0xF` on a source.
    pub write_mask: u8,
    pub modifier: SrcMod,
    /// Clamps the result to `[0,1]`.
    pub saturate: bool,
    /// `c[a0.x + index]`, so the address register offsets the index.
    pub relative: bool,
    /// Which component of the address register indexes a relative operand.
    pub rel_component: u8,
}

#[derive(Clone, Debug)]
pub struct Inst {
    pub opcode: u16,
    /// e.g. the comparison for `ifc`/`breakc`/`setp`.
    pub control: u8,
    pub dst: Option<Operand>,
    pub src: Vec<Operand>,
    /// For `def`.
    pub def: Option<[f32; 4]>,
    /// For `defi`; a `rep` loop count lives in `.x`.
    pub idef: Option<[i32; 4]>,
    /// For `dcl`; bits `[30:27]` are the texture type on a sampler.
    pub dcl: Option<u32>,
}

/// D3DSAMPLER_TEXTURE_TYPE, from bits `[30:27]` of a `dcl` token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SamplerDim {
    Tex2D,
    Cube,
    Volume,
    Unknown,
}

impl SamplerDim {
    pub fn from_dcl(tok: u32) -> SamplerDim {
        match (tok >> 27) & 0xf {
            2 => SamplerDim::Tex2D,
            3 => SamplerDim::Cube,
            4 => SamplerDim::Volume,
            _ => SamplerDim::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Constant {
    pub name: String,
    /// 0=bool, 1=int4, 2=float4, 3=sampler.
    pub reg_set: u8,
    pub reg_index: u16,
    pub reg_count: u16,
    pub default: Option<Vec<f32>>,
}

#[derive(Clone, Debug)]
pub struct Shader {
    pub pixel: bool,
    pub major: u8,
    pub minor: u8,
    pub insts: Vec<Inst>,
    pub constants: Vec<Constant>,
}

impl Shader {
    /// Maps e.g. `c190` to "shininess".
    pub fn const_name(&self, kind: RegKind, index: u16) -> Option<&str> {
        let set = match kind {
            RegKind::Const => 2,
            RegKind::Sampler => 3,
            RegKind::ConstInt => 1,
            RegKind::ConstBool => 0,
            _ => return None,
        };
        self.constants
            .iter()
            .find(|c| {
                c.reg_set == set && index >= c.reg_index && index < c.reg_index + c.reg_count.max(1)
            })
            .map(|c| c.name.as_str())
    }
}

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn cstr(d: &[u8], o: usize) -> String {
    let Some(s) = d.get(o..) else {
        return String::new();
    };
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).into_owned()
}

/// The register type is split across bits [30:28] and [12:11] of the token.
fn reg_kind(tok: u32) -> RegKind {
    RegKind::from_code((((tok >> 28) & 0x7) | ((tok >> 8) & 0x18)) as u8)
}

fn decode_dst(tok: u32) -> Operand {
    Operand {
        kind: reg_kind(tok),
        index: (tok & 0x7ff) as u16,
        swizzle: [0, 1, 2, 3],
        write_mask: ((tok >> 16) & 0xf) as u8,
        modifier: SrcMod::None,
        saturate: (tok & 0x0010_0000) != 0,
        relative: (tok & (1 << 13)) != 0,
        rel_component: 0,
    }
}

fn decode_src(tok: u32) -> Operand {
    let sw = (tok >> 16) & 0xff;
    Operand {
        kind: reg_kind(tok),
        index: (tok & 0x7ff) as u16,
        swizzle: [
            (sw & 0x3) as u8,
            ((sw >> 2) & 0x3) as u8,
            ((sw >> 4) & 0x3) as u8,
            ((sw >> 6) & 0x3) as u8,
        ],
        write_mask: 0xf,
        modifier: SrcMod::from_code(((tok >> 24) & 0xf) as u8),
        saturate: false,
        relative: (tok & (1 << 13)) != 0,
        rel_component: 0,
    }
}

/// `blob` runs from the version token to the end token.
pub fn decode(blob: &[u8]) -> Result<Shader> {
    if blob.len() < 8 {
        return Err(malformed("shader blob too short"));
    }
    let version = u32le(blob, 0);
    let pixel = match version {
        PS_3_0 => true,
        VS_3_0 => false,
        _ => return Err(malformed("not a vs_3_0/ps_3_0 shader")),
    };
    let mut insts = Vec::new();
    let mut constants = Vec::new();
    let mut p = 4;
    while p + 4 <= blob.len() {
        let tok = u32le(blob, p);
        let op = (tok & 0xffff) as u16;
        if op == END_OP {
            break;
        }
        if op == COMMENT_OP {
            let len = ((tok >> 16) & 0x7fff) as usize;
            if blob.len() >= p + 8 && &blob[p + 4..p + 8] == b"CTAB" {
                constants = parse_ctab(blob, p + 4);
            }
            p += 4 + len * 4;
            continue;
        }
        let len = ((tok >> 24) & 0xf) as usize;
        if p + 4 + len * 4 > blob.len() {
            break;
        }
        let words: Vec<u32> = (0..len).map(|k| u32le(blob, p + 4 + k * 4)).collect();
        p += 4 + len * 4;
        let control = ((tok >> 16) & 0xff) as u8;

        if op == OP_DEF && words.len() >= 5 {
            insts.push(Inst {
                opcode: op,
                control,
                dst: Some(decode_dst(words[0])),
                src: Vec::new(),
                def: Some([
                    f32::from_bits(words[1]),
                    f32::from_bits(words[2]),
                    f32::from_bits(words[3]),
                    f32::from_bits(words[4]),
                ]),
                idef: None,
                dcl: None,
            });
            continue;
        }
        if op == OP_DEFI && words.len() >= 5 {
            insts.push(Inst {
                opcode: op,
                control,
                dst: Some(decode_dst(words[0])),
                src: Vec::new(),
                def: None,
                idef: Some([
                    words[1] as i32,
                    words[2] as i32,
                    words[3] as i32,
                    words[4] as i32,
                ]),
                dcl: None,
            });
            continue;
        }
        if op == OP_DCL {
            let dst = words.get(1).map(|&w| decode_dst(w));
            insts.push(Inst {
                opcode: op,
                control,
                dst,
                src: Vec::new(),
                def: None,
                idef: None,
                dcl: words.first().copied(),
            });
            continue;
        }
        let (dst, src) = split_operands(op, &words);
        insts.push(Inst {
            opcode: op,
            control,
            dst,
            src,
            def: None,
            idef: None,
            dcl: None,
        });
    }
    Ok(Shader {
        pixel,
        major: 3,
        minor: 0,
        insts,
        constants,
    })
}

/// Their tokens are all source operands.
fn has_no_dst(op: u16) -> bool {
    matches!(
        op,
        25 | 26 | 27 | 28 | 29 | 38 | 39 | 40 | 41 | 42 | 43 | 44 | 45 | 65
    )
}

fn split_operands(op: u16, words: &[u32]) -> (Option<Operand>, Vec<Operand>) {
    // A relative-addressed operand is followed by an extra token, so positional indexing would
    // misalign every later source.
    let mut idx = 0usize;
    let dst = if has_no_dst(op) {
        None
    } else if idx < words.len() {
        let mut d = decode_dst(words[idx]);
        idx += 1;
        if d.relative && idx < words.len() {
            d.rel_component = decode_src(words[idx]).swizzle[0];
            idx += 1;
        }
        Some(d)
    } else {
        None
    };
    let mut src = Vec::new();
    while idx < words.len() {
        let mut s = decode_src(words[idx]);
        idx += 1;
        if s.relative && idx < words.len() {
            s.rel_component = decode_src(words[idx]).swizzle[0];
            idx += 1;
        }
        src.push(s);
    }
    (dst, src)
}

/// `ctab` is the offset of the fourcc; offsets inside are relative to the byte after it.
fn parse_ctab(d: &[u8], ctab: usize) -> Vec<Constant> {
    let base = ctab + 4;
    if base + 28 > d.len() {
        return Vec::new();
    }
    let count = u32le(d, base + 12) as usize;
    let info = base + u32le(d, base + 16) as usize;
    let mut out = Vec::with_capacity(count.min(d.len() / 20));
    for i in 0..count {
        let e = info + i * 20;
        if e + 20 > d.len() {
            break;
        }
        let name = cstr(d, base + u32le(d, e) as usize);
        let reg_set = (u32le(d, e + 4) & 0xff) as u8;
        let reg_index = u16::from_le_bytes([d[e + 6], d[e + 7]]);
        let reg_count = u16::from_le_bytes([d[e + 8], d[e + 9]]);
        let default_off = u32le(d, e + 16) as usize;
        let default = (default_off != 0 && reg_set == 2).then(|| {
            (0..reg_count as usize * 4)
                .map(|k| {
                    let o = base + default_off + k * 4;
                    if o + 4 <= d.len() {
                        f32::from_bits(u32le(d, o))
                    } else {
                        0.0
                    }
                })
                .collect()
        });
        out.push(Constant {
            name,
            reg_set,
            reg_index,
            reg_count,
            default,
        });
    }
    out
}

pub fn find_shaders(material: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 4 <= material.len() {
        let v = u32le(material, o);
        if v == VS_3_0 || v == PS_3_0 {
            let mut e = o + 4;
            while e + 4 <= material.len() && u32le(material, e) != END_TOKEN {
                e += 4;
            }
            let end = (e + 4).min(material.len());
            out.push(&material[o..end]);
            o = end;
        } else {
            o += 4;
        }
    }
    out
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "D3D9 shader",
        detail: detail.to_string(),
    }
}

/// The game packs the primary UV in a varying's `.xy` and the secondary in its `.zw`, so a `texld`
/// swizzled to start at z/w is reading the set detail `_d` maps tile on.
pub fn detail_samplers(ps: &Shader) -> std::collections::BTreeSet<u16> {
    let mut out = std::collections::BTreeSet::new();
    for inst in &ps.insts {
        if inst.opcode == op::TEXLD
            && let (Some(coord), Some(samp)) = (inst.src.first(), inst.src.get(1))
            && coord.kind == RegKind::Input
            && coord.swizzle.first().is_some_and(|&c| c >= 2)
        {
            out.insert(samp.index);
        }
    }
    out
}

/// What a `_sampler_NN` is for, recovered from how the shader consumes its texels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Normal,
    DetailNormal,
    Param,
    Color,
}

/// Tracks which texels flow into a tell-tale constant: `normalPower` means normal,
/// `multiNormalPower` detail normal, `*shininess` spec. Samplers with no signal stay unclassified.
pub fn sampler_roles(ps: &Shader) -> std::collections::HashMap<u16, Role> {
    use std::collections::{BTreeSet, HashMap};
    let mut carry: HashMap<u16, BTreeSet<u16>> = HashMap::new();
    let mut roles: HashMap<u16, Role> = HashMap::new();
    for inst in &ps.insts {
        let mut srcset: BTreeSet<u16> = BTreeSet::new();
        for s in &inst.src {
            if s.kind == RegKind::Temp
                && let Some(set) = carry.get(&s.index)
            {
                srcset.extend(set.iter().copied());
            }
        }
        for s in &inst.src {
            if s.kind == RegKind::Const {
                let role = match ps.const_name(RegKind::Const, s.index) {
                    Some("normalPower") => Some(Role::Normal),
                    Some("multiNormalPower") => Some(Role::DetailNormal),
                    Some(n) if n.contains("hininess") => Some(Role::Param),
                    _ => None,
                };
                if let Some(role) = role {
                    for &samp in &srcset {
                        roles.entry(samp).or_insert(role);
                    }
                }
            }
        }
        if inst.opcode == op::TEXLD {
            if let (Some(dst), Some(samp)) = (inst.dst, inst.src.get(1))
                && dst.kind == RegKind::Temp
            {
                carry.insert(dst.index, BTreeSet::from([samp.index]));
            }
        } else if let Some(dst) = inst.dst
            && dst.kind == RegKind::Temp
        {
            carry.insert(dst.index, srcset);
        }
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_minimal_pixel_shader() {
        let mut b: Vec<u8> = Vec::new();
        let mut dw = |v: u32| b.extend_from_slice(&v.to_le_bytes());
        dw(PS_3_0);
        // def c0, 1,2,3,4
        dw(81 | (5 << 24));
        dw(0x2000_0000);
        dw(1.0f32.to_bits());
        dw(2.0f32.to_bits());
        dw(3.0f32.to_bits());
        dw(4.0f32.to_bits());
        // mov oC0, c0
        dw(1 | (2 << 24));
        dw(0x0000_0800 | (0xF << 16));
        dw(0x2000_0000 | (0xE4 << 16));
        dw(END_TOKEN);

        let s = decode(&b).unwrap();
        assert!(s.pixel);
        assert_eq!(s.insts.len(), 2);
        let def = &s.insts[0];
        assert_eq!(def.opcode, 81);
        assert_eq!(def.def, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(def.dst.unwrap().kind, RegKind::Const);
        let mov = &s.insts[1];
        assert_eq!(mov.opcode, 1);
        assert_eq!(mov.dst.unwrap().kind, RegKind::ColorOut);
        assert_eq!(mov.src[0].kind, RegKind::Const);
        assert_eq!(mov.src[0].swizzle, [0, 1, 2, 3]);
    }

    #[test]
    fn decodes_skin_shader_from_game() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/chr/pc/c001/bin/c001.win32.trb");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let trb = crate::trb::Trb::parse(&bytes).unwrap();
        let mut found_main = false;
        for i in 0..trb.resource_count() {
            let d = trb.resource_data(i).unwrap_or(&[]);
            if !d.starts_with(b"SEDBshd")
                || !trb
                    .resource_names()
                    .get(i)
                    .map(|n| n.contains("3skin"))
                    .unwrap_or(false)
            {
                continue;
            }
            for blob in find_shaders(d) {
                let sh = decode(blob).unwrap();
                if sh.pixel && sh.constants.iter().any(|c| c.name == "lightToneMap") {
                    assert!(
                        sh.insts.len() > 50,
                        "real shader should have many instructions"
                    );
                    assert!(sh.constants.iter().any(|c| c.reg_set == 3), "has samplers");
                    found_main = true;
                }
            }
        }
        assert!(found_main, "should find the skin pixel shader");
    }
}
