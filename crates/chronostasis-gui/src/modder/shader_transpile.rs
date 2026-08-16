use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use ff13::formats::d3d9shader::{Inst, Operand, RegKind, SamplerDim, Shader, SrcMod, op};
use ff13::formats::model::MAX_PALETTE;

pub struct Transpiled {
    pub wgsl: String,
    pub const_count: usize,
    pub samplers: Vec<(u16, String, SamplerDim)>,
    pub vs_wgsl: String,
    pub vs_const_count: usize,
}

pub fn transpile(sh: &Shader, color_samplers: &BTreeSet<u16>) -> Result<Transpiled, String> {
    if !sh.pixel {
        return Err("not a pixel shader".into());
    }
    let mut info = collect(sh);
    info.color_samplers = color_samplers.clone();
    let raw = std::env::var("FF13_GAMMA").as_deref() == Ok("raw");
    info.gamma_linearize = !raw;
    info.gamma_encode = !raw;

    let mut w = String::new();
    let used = info.const_count.max(1);
    let nc = used + 1;
    let _ = writeln!(w, "struct Consts {{ c: array<vec4<f32>, {nc}>, }};");
    let _ = writeln!(w, "@group(0) @binding(1) var<uniform> K: Consts;");
    let _ = writeln!(
        w,
        "@group(0) @binding(2) var<uniform> Bv: array<vec4<u32>, 4>;"
    );
    let _ = writeln!(
        w,
        "fn bget(n: u32) -> bool {{ return Bv[n >> 2u][n & 3u] != 0u; }}"
    );
    let _ = writeln!(
        w,
        "fn s2l4(c: vec4<f32>) -> vec4<f32> {{ return vec4<f32>(pow(c.rgb, vec3<f32>(2.2)), c.a); }}"
    );

    let mut binding = 0u32;
    for &(idx, _, dim) in &info.samplers {
        let tex = match dim {
            SamplerDim::Cube => "texture_cube<f32>",
            SamplerDim::Volume => "texture_3d<f32>",
            _ => "texture_2d<f32>",
        };
        let _ = writeln!(w, "@group(1) @binding({binding}) var t{idx}: {tex};");
        binding += 1;
        let _ = writeln!(w, "@group(1) @binding({binding}) var smp{idx}: sampler;");
        binding += 1;
    }

    let _ = writeln!(w, "struct FragIn {{");
    if !info.misc.is_empty() {
        let _ = writeln!(w, "  @builtin(position) frag_pos: vec4<f32>,");
        let _ = writeln!(w, "  @builtin(front_facing) front_facing: bool,");
    }
    for &v in &info.inputs {
        let _ = writeln!(w, "  @location({v}) v{v}: vec4<f32>,");
    }
    if info.inputs.is_empty() && info.misc.is_empty() {
        let _ = writeln!(w, "  @location(0) v0: vec4<f32>,");
    }
    let _ = writeln!(w, "}};");

    let _ = writeln!(w, "@fragment");
    let _ = writeln!(w, "fn fs_main(vin: FragIn) -> @location(0) vec4<f32> {{");
    for t in 0..=info.max_temp {
        let _ = writeln!(w, "  var r{t}: vec4<f32> = vec4<f32>(0.0);");
    }
    for &m in &info.misc {
        let init = match m {
            0 => "vin.frag_pos".to_string(),
            1 => "vec4<f32>(select(-1.0, 1.0, vin.front_facing))".to_string(),
            _ => "vec4<f32>(1.0)".to_string(),
        };
        let _ = writeln!(w, "  var vmisc{m}: vec4<f32> = {init};");
    }
    for &o in &info.color_out {
        let _ = writeln!(w, "  var oC{o}: vec4<f32> = vec4<f32>(0.0);");
    }
    if !info.color_out.contains(&0) {
        let _ = writeln!(w, "  var oC0: vec4<f32> = vec4<f32>(0.0);");
    }
    if info.depth_out {
        let _ = writeln!(w, "  var oDepth: f32 = 0.0;");
    }

    let insts = unroll_loops(&sh.insts);
    let mut depth = 0usize;
    let mut tmp = 0usize;
    let dbg_inst: Option<usize> = std::env::var("FF13_DBG_INST")
        .ok()
        .and_then(|s| s.parse().ok());
    for (i, inst) in insts.iter().enumerate() {
        emit_inst(&mut w, inst, &info, &mut depth, &mut tmp)?;
        if Some(i) == dbg_inst {
            let name = match inst.dst {
                Some(d) if matches!(d.kind, RegKind::ColorOut) => format!("oC{}", d.index),
                Some(d) => format!("r{}", d.index),
                None => "oC0".to_string(),
            };
            eprintln!(
                "FF13_DBG_INST {i} op={} -> {name} (of {} insts)",
                inst.opcode,
                insts.len()
            );
            for _ in 0..depth {
                let _ = writeln!(w, "  }}");
            }
            let _ = writeln!(
                w,
                "  return vec4<f32>(clamp({name}.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);"
            );
            let _ = writeln!(w, "}}");
            return Ok(Transpiled {
                wgsl: w,
                const_count: used,
                samplers: info.samplers,
                vs_wgsl: String::new(),
                vs_const_count: 0,
            });
        }
    }

    if let Ok(reg) = std::env::var("FF13_DBG_REG") {
        let reg = if reg == "oC0" {
            "oC0".to_string()
        } else if reg.starts_with('v') {
            format!("vin.{reg}")
        } else {
            format!("r{reg}")
        };
        let expr = match std::env::var("FF13_DBG_MODE").as_deref() {
            Ok("len") => format!("vec3<f32>(length({reg}.xyz) * 0.5)"),
            Ok("abs") => format!("abs({reg}.rgb)"),
            _ => format!("clamp({reg}.rgb, vec3<f32>(0.0), vec3<f32>(1.0))"),
        };
        let _ = writeln!(w, "  return vec4<f32>({expr}, 1.0);");
    } else if info.gamma_encode {
        let _ = writeln!(
            w,
            "  return vec4<f32>(pow(max(oC0.rgb, vec3<f32>(0.0)), vec3<f32>(K.c[{used}u].x)), oC0.a);"
        );
    } else {
        let _ = writeln!(w, "  return oC0;");
    }
    let _ = writeln!(w, "}}");

    Ok(Transpiled {
        wgsl: w,
        const_count: used,
        samplers: info.samplers,
        vs_wgsl: String::new(),
        vs_const_count: 0,
    })
}

fn vertex_stage(ps: &Shader) -> String {
    use std::collections::BTreeSet;
    let mut uv_regs: BTreeSet<u16> = BTreeSet::new();
    let mut max_in = 6u16;
    for inst in &ps.insts {
        if inst.opcode == op::TEXLD
            && let Some(coord) = inst.src.first()
            && coord.kind == RegKind::Input
        {
            uv_regs.insert(coord.index);
        }
        for s in &inst.src {
            if s.kind == RegKind::Input {
                max_in = max_in.max(s.index);
            }
        }
    }
    let mut w = String::new();
    let _ = writeln!(w, "struct VsU {{ mvp: mat4x4<f32>, eye: vec4<f32>, }};");
    let _ = writeln!(w, "@group(0) @binding(0) var<uniform> VU: VsU;");
    let _ = writeln!(
        w,
        "@group(2) @binding(0) var<uniform> Bones: array<mat4x4<f32>, {MAX_PALETTE}>;"
    );
    let _ = writeln!(w, "struct VOut {{");
    let _ = writeln!(w, "  @builtin(position) pos: vec4<f32>,");
    for loc in 0u16..=max_in {
        let _ = writeln!(w, "  @location({loc}) v{loc}: vec4<f32>,");
    }
    let _ = writeln!(w, "}};");
    let _ = writeln!(w, "@vertex");
    let _ = writeln!(
        w,
        "fn vs_main(@location(0) a_pos: vec3<f32>, @location(1) a_normal: vec3<f32>, @location(2) a_tangent: vec4<f32>, @location(3) a_uv: vec2<f32>, @location(4) a_color: vec4<f32>, @location(5) a_uv1: vec2<f32>, @location(6) a_joints: vec4<u32>, @location(7) a_weights: vec4<f32>) -> VOut {{"
    );
    let _ = writeln!(w, "  var o: VOut;");
    let _ = writeln!(w, "  var sp = a_pos;");
    let _ = writeln!(w, "  var snrm = a_normal;");
    let _ = writeln!(w, "  var stan = a_tangent.xyz;");
    let _ = writeln!(
        w,
        "  let wsum = a_weights.x + a_weights.y + a_weights.z + a_weights.w;"
    );
    let _ = writeln!(w, "  if (wsum > 0.001) {{");
    let _ = writeln!(
        w,
        "    let m = a_weights.x * Bones[a_joints.x] + a_weights.y * Bones[a_joints.y] + a_weights.z * Bones[a_joints.z] + a_weights.w * Bones[a_joints.w];"
    );
    let _ = writeln!(w, "    sp = (m * vec4<f32>(a_pos, 1.0)).xyz;");
    let _ = writeln!(w, "    let r = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);");
    let _ = writeln!(w, "    snrm = r * a_normal;");
    let _ = writeln!(w, "    stan = r * a_tangent.xyz;");
    let _ = writeln!(w, "  }}");
    let _ = writeln!(w, "  let wp = vec4<f32>(sp, 1.0);");
    let _ = writeln!(w, "  o.pos = VU.mvp * wp;");
    let _ = writeln!(
        w,
        "  let uv = vec4<f32>(a_uv.x, 1.0 - a_uv.y, a_uv.x, 1.0 - a_uv.y);"
    );
    let _ = writeln!(
        w,
        "  let uv1 = vec4<f32>(a_uv1.x, 1.0 - a_uv1.y, a_uv1.x, 1.0 - a_uv1.y);"
    );
    // Some materials read both UV sets from one register, primary in `.xy` and secondary in `.zw`.
    let _ = writeln!(
        w,
        "  let uvc = vec4<f32>(a_uv.x, 1.0 - a_uv.y, a_uv1.x, 1.0 - a_uv1.y);"
    );
    // The lowest coord register is the primary UV; any further one is a detail map.
    let primary_uv = uv_regs.iter().next().copied();
    for loc in 0u16..=max_in {
        let expr = if uv_regs.contains(&loc) {
            if Some(loc) == primary_uv {
                "uvc".to_string()
            } else {
                "uv1".to_string()
            }
        } else {
            match loc {
                0 => "a_color".into(),
                1 => "wp".into(),
                2 => "o.pos".into(),
                3 => "vec4<f32>(snrm, 0.0)".into(),
                // The shader unpacks the tangent with `v*2-1`, so output it packed to [0,1].
                4 => "(vec4<f32>(stan, a_tangent.w) * 0.5 + vec4<f32>(0.5))".into(),
                // v5/v6 are direction slots (view direction), not UV.
                5 | 6 => "vec4<f32>(normalize(VU.eye.xyz - sp), 0.0)".into(),
                _ => "uv".into(),
            }
        };
        let _ = writeln!(w, "  o.v{loc} = {expr};");
    }
    let _ = writeln!(w, "  return o;");
    let _ = writeln!(w, "}}");
    w
}

fn vs_attr_for(usage: u8, index: u8) -> &'static str {
    match (usage, index) {
        (0, 0) => "vec4<f32>(a_pos, 1.0)",
        (3, 0) => "vec4<f32>(a_normal, 0.0)",
        (10, 0) => "a_color",
        (5, 0) => "vec4<f32>(a_uv, 0.0, 0.0)",
        (5, 1) => "vec4<f32>(a_uv1, 0.0, 0.0)",
        (5, 5) => "a_tangent",
        (5, 6) => "a_weights",
        (5, 7) => "vec4<f32>(a_joints)",
        (6, 0) => "a_tangent",
        (1, 0) => "a_weights",
        (2, 0) => "vec4<f32>(a_joints)",
        _ => "vec4<f32>(0.0)",
    }
}

fn dcls(sh: &Shader, kind: RegKind) -> Vec<(u16, u8, u8)> {
    sh.insts
        .iter()
        .filter_map(|inst| {
            if inst.opcode != op::DCL {
                return None;
            }
            let (tok, dst) = (inst.dcl?, inst.dst?);
            (dst.kind == kind).then_some((dst.index, (tok & 0x1f) as u8, ((tok >> 16) & 0xf) as u8))
        })
        .collect()
}

fn transpile_vertex(vs: &Shader, ps: &Shader) -> Result<(String, usize), String> {
    use std::collections::BTreeMap;
    if vs.pixel {
        return Err("not a vertex shader".into());
    }
    let ps_in: BTreeMap<u16, (u8, u8)> = dcls(ps, RegKind::Input)
        .into_iter()
        .map(|(r, u, i)| (r, (u, i)))
        .collect();
    let ps_regs = collect(ps).inputs;
    let mut out_sem: BTreeMap<(u8, u8), u16> = BTreeMap::new();
    let mut pos_reg = None;
    for (r, u, i) in dcls(vs, RegKind::Output) {
        if u == 0 {
            pos_reg = Some(r);
        } else {
            out_sem.insert((u, i), r);
        }
    }
    let pos_reg = pos_reg.ok_or("VS has no position output")?;

    let mut info = collect(vs);
    info.vertex = true;
    info.vs_inputs = dcls(vs, RegKind::Input)
        .into_iter()
        .map(|(r, u, i)| (r, vs_attr_for(u, i).to_string()))
        .collect();
    let nc = vs
        .constants
        .iter()
        .filter(|c| c.reg_set == 2)
        .map(|c| (c.reg_index + c.reg_count.max(1)) as usize)
        .max()
        .unwrap_or(1)
        .max(1);
    info.const_count = nc;
    for con in vs.constants.iter().filter(|c| c.reg_set == 2) {
        let n = con.name.to_ascii_lowercase();
        if n.contains("worldviewproj") {
            info.vs_wvp_base = Some(con.reg_index);
        } else if n == "worldviewmatrix" {
            info.vs_wv_base = Some(con.reg_index);
        } else if n.contains("viewit") {
            info.vs_viewit_base = Some(con.reg_index);
        } else if n.contains("joint") || n.contains("bonematri") {
            info.vs_jm = Some((con.reg_index, con.reg_count));
        }
    }
    let max_out = vs
        .insts
        .iter()
        .flat_map(|inst| inst.dst.iter().chain(inst.src.iter()))
        .filter(|o| o.kind == RegKind::Output)
        .map(|o| o.index)
        .max()
        .unwrap_or(pos_reg)
        .max(pos_reg);

    let mut w = String::new();
    let _ = writeln!(
        w,
        "struct VsU {{ mvp: mat4x4<f32>, eye: vec4<f32>, view: mat4x4<f32>, viewit: mat4x4<f32>, }};"
    );
    let _ = writeln!(w, "@group(0) @binding(0) var<uniform> VU: VsU;");
    let _ = writeln!(w, "struct C {{ c: array<vec4<f32>, {nc}>, }};");
    let _ = writeln!(w, "@group(0) @binding(3) var<uniform> K: C;");
    let _ = writeln!(
        w,
        "@group(0) @binding(4) var<uniform> Bv: array<vec4<u32>, 4>;"
    );
    let _ = writeln!(
        w,
        "fn bget(n: u32) -> bool {{ return Bv[n >> 2u][n & 3u] != 0u; }}"
    );
    let _ = writeln!(
        w,
        "@group(2) @binding(0) var<uniform> Bones: array<mat4x4<f32>, {MAX_PALETTE}>;"
    );
    let _ = writeln!(w, "struct VOut {{");
    let _ = writeln!(w, "  @builtin(position) pos: vec4<f32>,");
    for &l in &ps_regs {
        let _ = writeln!(w, "  @location({l}) v{l}: vec4<f32>,");
    }
    let _ = writeln!(w, "}};");
    let _ = writeln!(w, "@vertex");
    let _ = writeln!(
        w,
        "fn vs_main(@location(0) a_pos: vec3<f32>, @location(1) a_normal: vec3<f32>, @location(2) a_tangent: vec4<f32>, @location(3) a_uv: vec2<f32>, @location(4) a_color: vec4<f32>, @location(5) a_uv1: vec2<f32>, @location(6) a_joints: vec4<u32>, @location(7) a_weights: vec4<f32>) -> VOut {{"
    );
    let _ = writeln!(w, "  var a0: vec4<i32> = vec4<i32>(0);");
    for t in 0..=info.max_temp {
        let _ = writeln!(w, "  var r{t}: vec4<f32> = vec4<f32>(0.0);");
    }
    for o in 0..=max_out {
        let _ = writeln!(w, "  var o{o}: vec4<f32> = vec4<f32>(0.0);");
    }
    let (mut depth, mut tmp) = (0usize, 0usize);
    for inst in &vs.insts {
        emit_inst(&mut w, inst, &info, &mut depth, &mut tmp)?;
    }
    let _ = writeln!(w, "  var out: VOut;");
    let _ = writeln!(w, "  out.pos = o{pos_reg};");
    for &l in &ps_regs {
        let expr = ps_in
            .get(&l)
            .and_then(|sem| out_sem.get(sem))
            .map(|r| format!("o{r}"))
            .unwrap_or_else(|| "vec4<f32>(0.0)".into());
        let _ = writeln!(w, "  out.v{l} = {expr};");
    }
    let _ = writeln!(w, "  return out;");
    let _ = writeln!(w, "}}");
    Ok((w, nc))
}

pub fn full_module(
    ps: &Shader,
    color_samplers: &BTreeSet<u16>,
    vs: Option<&Shader>,
) -> Result<Transpiled, String> {
    let mut t = transpile(ps, color_samplers)?;
    match vs.and_then(|vs| transpile_vertex(vs, ps).ok()) {
        Some((vs_wgsl, nc)) => {
            t.vs_wgsl = vs_wgsl;
            t.vs_const_count = nc;
        }
        None => t.wgsl = format!("{}\n{}", vertex_stage(ps), t.wgsl),
    }
    Ok(t)
}

struct Info {
    max_temp: u16,
    inputs: Vec<u16>,
    misc: Vec<u16>,
    color_out: Vec<u16>,
    depth_out: bool,
    const_count: usize,
    defs: BTreeMap<u16, [f32; 4]>,
    sampler_dim: BTreeMap<u16, SamplerDim>,
    samplers: Vec<(u16, String, SamplerDim)>,
    color_samplers: BTreeSet<u16>,
    gamma_linearize: bool,
    gamma_encode: bool,
    vertex: bool,
    vs_inputs: BTreeMap<u16, String>,
    vs_wvp_base: Option<u16>,
    vs_wv_base: Option<u16>,
    vs_viewit_base: Option<u16>,
    vs_jm: Option<(u16, u16)>,
}

fn collect(sh: &Shader) -> Info {
    let mut max_temp = 0u16;
    let mut inputs = BTreeSet::new();
    let mut misc = BTreeSet::new();
    let mut color_out = BTreeSet::new();
    let mut depth_out = false;
    let mut max_const = 0i32;
    let mut defs = BTreeMap::new();
    let mut sampler_dim = BTreeMap::new();
    let mut sampler_set = BTreeSet::new();

    let mut note = |op: &Operand, max_temp: &mut u16, max_const: &mut i32| match op.kind {
        RegKind::Temp => *max_temp = (*max_temp).max(op.index),
        RegKind::Input => {
            inputs.insert(op.index);
        }
        RegKind::Misc => {
            misc.insert(op.index);
        }
        RegKind::ColorOut => {
            color_out.insert(op.index);
        }
        RegKind::DepthOut => depth_out = true,
        RegKind::Const if !op.relative => *max_const = (*max_const).max(op.index as i32),
        _ => {}
    };

    for inst in &sh.insts {
        if inst.opcode == op::DEF {
            if let (Some(dst), Some(d)) = (inst.dst, inst.def) {
                defs.insert(dst.index, d);
            }
            continue;
        }
        // `defi` (integer const) carries garbage "src" in our generic decode; skip it so it
        // doesn't bump max_const/max_temp. The int values feed `rep` counts, handled in transpile.
        if inst.opcode == op::DEFI {
            continue;
        }
        if inst.opcode == op::DCL {
            if let Some(dst) = inst.dst
                && dst.kind == RegKind::Sampler
            {
                let dim = inst
                    .dcl
                    .map(SamplerDim::from_dcl)
                    .unwrap_or(SamplerDim::Tex2D);
                sampler_dim.insert(dst.index, dim);
            }
            continue;
        }
        if let Some(dst) = inst.dst {
            note(&dst, &mut max_temp, &mut max_const);
        }
        for s in &inst.src {
            if s.kind == RegKind::Sampler {
                sampler_set.insert(s.index);
            } else {
                note(s, &mut max_temp, &mut max_const);
            }
        }
    }

    let const_count = (max_const + 1).max(0) as usize;

    let samplers: Vec<(u16, String, SamplerDim)> = sampler_set
        .iter()
        .map(|&i| {
            let dim = sampler_dim.get(&i).copied().unwrap_or(SamplerDim::Tex2D);
            let name = sh.const_name(RegKind::Sampler, i).unwrap_or("").to_string();
            (i, name, dim)
        })
        .collect();

    Info {
        max_temp,
        inputs: inputs.into_iter().collect(),
        misc: misc.into_iter().collect(),
        color_out: color_out.into_iter().collect(),
        depth_out,
        const_count,
        defs,
        sampler_dim,
        samplers,
        color_samplers: BTreeSet::new(),
        gamma_linearize: true,
        gamma_encode: true,
        vertex: false,
        vs_inputs: BTreeMap::new(),
        vs_wvp_base: None,
        vs_wv_base: None,
        vs_viewit_base: None,
        vs_jm: None,
    }
}

fn wf(f: f32) -> String {
    if f.is_nan() {
        return "0.0".into();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "-3.0e38".into()
        } else {
            "3.0e38".into()
        };
    }
    let s = format!("{f:?}");
    s
}

fn swz(op: &Operand) -> String {
    if op.swizzle == [0, 1, 2, 3] {
        return String::new();
    }
    let c = ['x', 'y', 'z', 'w'];
    format!(
        ".{}{}{}{}",
        c[op.swizzle[0] as usize],
        c[op.swizzle[1] as usize],
        c[op.swizzle[2] as usize],
        c[op.swizzle[3] as usize]
    )
}

fn src(op: &Operand, info: &Info) -> String {
    let base = match op.kind {
        RegKind::Temp => format!("r{}", op.index),
        RegKind::Input if info.vertex => info
            .vs_inputs
            .get(&op.index)
            .cloned()
            .unwrap_or_else(|| "vec4<f32>(0.0)".into()),
        RegKind::Input => format!("vin.v{}", op.index),
        RegKind::Misc => format!("vmisc{}", op.index),
        RegKind::ColorOut => format!("oC{}", op.index),
        RegKind::Output if info.vertex => format!("o{}", op.index),
        RegKind::Const => {
            let c = ['x', 'y', 'z', 'w'][op.rel_component as usize];
            let in_jm = info
                .vs_jm
                .filter(|&(b, n)| op.index >= b && op.index < b + n);
            let in_wvp = info
                .vs_wvp_base
                .filter(|&b| op.index >= b && op.index < b + 4);
            if let Some(d) = info.defs.get(&op.index) {
                format!(
                    "vec4<f32>({}, {}, {}, {})",
                    wf(d[0]),
                    wf(d[1]),
                    wf(d[2]),
                    wf(d[3])
                )
            } else if let (true, Some((base, _))) = (op.relative, in_jm) {
                // jointMatrices row: bone = a0/3 (3 regs per 4x3 matrix), transposed for the VS's dp4, clamped in range.
                let last_bone = MAX_PALETTE - 1;
                format!(
                    "transpose(Bones[min(u32(max(a0.{c}, 0i)) / 3u, {last_bone}u)])[{}u]",
                    op.index - base
                )
            } else if let Some(base) = in_wvp.filter(|_| !op.relative) {
                // WVP column (MAD form): each register is a matrix column, so index MVP directly (no transpose).
                format!("VU.mvp[{}u]", op.index - base)
            } else if let Some(base) = info
                .vs_wv_base
                .filter(|&b| !op.relative && op.index >= b && op.index < b + 4)
            {
                format!("VU.view[{}u]", op.index - base)
            } else if let Some(base) = info
                .vs_viewit_base
                .filter(|&b| !op.relative && op.index >= b && op.index < b + 4)
            {
                format!("VU.viewit[{}u]", op.index - base)
            } else if op.relative {
                let last = info.const_count.saturating_sub(1);
                format!("K.c[u32(clamp(a0.{c} + {}i, 0i, {last}i))]", op.index)
            } else {
                format!("K.c[{}u]", op.index)
            }
        }
        _ => "vec4<f32>(0.0)".into(),
    };
    let v = format!("({base}{})", swz(op));
    match op.modifier {
        SrcMod::None => v,
        SrcMod::Neg => format!("(-{v})"),
        SrcMod::Bias => format!("({v} - vec4<f32>(0.5))"),
        SrcMod::BiasNeg => format!("(-({v} - vec4<f32>(0.5)))"),
        SrcMod::Sign => format!("({v} * 2.0 - 1.0)"),
        SrcMod::SignNeg => format!("(-({v} * 2.0 - 1.0))"),
        SrcMod::Comp => format!("(vec4<f32>(1.0) - {v})"),
        SrcMod::Abs => format!("abs({v})"),
        SrcMod::AbsNeg => format!("(-abs({v}))"),
        SrcMod::Other(_) => v,
    }
}

fn scalar(op: &Operand, info: &Info) -> String {
    format!("({}).x", src(op, info))
}

fn target(op: &Operand, info: &Info) -> String {
    match op.kind {
        RegKind::Temp => format!("r{}", op.index),
        RegKind::ColorOut => format!("oC{}", op.index),
        RegKind::DepthOut => "oDepth".into(),
        RegKind::Output if info.vertex => format!("o{}", op.index),
        RegKind::Output => format!("r{}", op.index),
        _ => "r0".into(),
    }
}

fn mask_comps(m: u8) -> Vec<char> {
    let c = ['x', 'y', 'z', 'w'];
    (0..4)
        .filter(|&i| m & (1 << i) != 0)
        .map(|i| c[i])
        .collect()
}

fn write_masked(w: &mut String, dst: &Operand, value: String, tmp: &mut usize, info: &Info) {
    let v = if dst.saturate {
        format!("clamp({value}, vec4<f32>(0.0), vec4<f32>(1.0))")
    } else {
        value
    };
    let id = *tmp;
    *tmp += 1;
    let _ = writeln!(w, "  let _t{id} = {v};");
    let tgt = target(dst, info);
    if dst.kind == RegKind::DepthOut {
        let _ = writeln!(w, "  {tgt} = _t{id}.x;");
        return;
    }
    let comps = mask_comps(dst.write_mask);
    let comps = if comps.is_empty() {
        vec!['x', 'y', 'z', 'w']
    } else {
        comps
    };
    for c in comps {
        let _ = writeln!(w, "  {tgt}.{c} = _t{id}.{c};");
    }
}

fn unroll_loops(insts: &[Inst]) -> Vec<Inst> {
    let mut counts: std::collections::HashMap<u16, i32> = std::collections::HashMap::new();
    for inst in insts {
        if inst.opcode == op::DEFI
            && let (Some(dst), Some(idef)) = (inst.dst, inst.idef)
        {
            counts.insert(dst.index, idef[0]);
        }
    }
    expand_loops(insts, &counts)
}

fn expand_loops(insts: &[Inst], counts: &std::collections::HashMap<u16, i32>) -> Vec<Inst> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < insts.len() {
        let inst = &insts[i];
        match inst.opcode {
            op::REP => {
                let count = inst
                    .src
                    .first()
                    .and_then(|s| counts.get(&s.index))
                    .copied()
                    .unwrap_or(0)
                    .clamp(0, 64) as usize;
                let (body, next) = loop_block(insts, i + 1);
                let flat = expand_loops(body, counts);
                for _ in 0..count {
                    out.extend(flat.iter().cloned());
                }
                i = next;
            }
            op::DEFI | op::ENDREP => i += 1,
            op::BREAK | op::BREAKC => i += 1,
            _ => {
                out.push(inst.clone());
                i += 1;
            }
        }
    }
    out
}

fn loop_block(insts: &[Inst], start: usize) -> (&[Inst], usize) {
    let mut depth = 1;
    let mut j = start;
    while j < insts.len() {
        match insts[j].opcode {
            op::REP => depth += 1,
            op::ENDREP => {
                depth -= 1;
                if depth == 0 {
                    return (&insts[start..j], j + 1);
                }
            }
            _ => {}
        }
        j += 1;
    }
    (&insts[start..], insts.len())
}

fn emit_inst(
    w: &mut String,
    inst: &Inst,
    info: &Info,
    depth: &mut usize,
    tmp: &mut usize,
) -> Result<(), String> {
    let o = inst.opcode;
    match o {
        op::DEF | op::DEFI | op::DCL | op::NOP => return Ok(()),
        op::IF => {
            let cond = cond_expr(inst, info);
            let _ = writeln!(w, "  if ({cond}) {{");
            *depth += 1;
            return Ok(());
        }
        op::ELSE => {
            let _ = writeln!(w, "  }} else {{");
            return Ok(());
        }
        op::ENDIF => {
            let _ = writeln!(w, "  }}");
            *depth = depth.saturating_sub(1);
            return Ok(());
        }
        op::TEXLD => return emit_texld(w, inst, info, *depth, tmp),
        op::TEXKILL => {
            if let Some(s) = inst.src.first() {
                let _ = writeln!(
                    w,
                    "  if (any(({}).xyz < vec3<f32>(0.0))) {{ discard; }}",
                    src(s, info)
                );
            }
            return Ok(());
        }
        op::MOVA => {
            if let Some(s) = inst.src.first() {
                let _ = writeln!(w, "  a0 = vec4<i32>(round({}));", src(s, info));
            }
            return Ok(());
        }
        _ => {}
    }

    let s = |i: usize| src(&inst.src[i], info);
    let value = match o {
        op::MOV => s(0),
        op::ADD => format!("({} + {})", s(0), s(1)),
        op::SUB => format!("({} - {})", s(0), s(1)),
        op::MUL => format!("({} * {})", s(0), s(1)),
        op::MAD => format!("({} * {} + {})", s(0), s(1), s(2)),
        op::MIN => format!("min({}, {})", s(0), s(1)),
        op::MAX => format!("max({}, {})", s(0), s(1)),
        op::LRP => format!("mix({}, {}, {})", s(2), s(1), s(0)),
        op::CMP => format!("select({}, {}, {} >= vec4<f32>(0.0))", s(2), s(1), s(0)),
        op::FRC => format!("fract({})", s(0)),
        op::ABS => format!("abs({})", s(0)),
        op::SLT => format!(
            "select(vec4<f32>(0.0), vec4<f32>(1.0), {} < {})",
            s(0),
            s(1)
        ),
        op::SGE => format!(
            "select(vec4<f32>(0.0), vec4<f32>(1.0), {} >= {})",
            s(0),
            s(1)
        ),
        op::RCP => format!("vec4<f32>(1.0 / {})", scalar(&inst.src[0], info)),
        op::RSQ => format!("vec4<f32>(1.0 / sqrt(abs({})))", scalar(&inst.src[0], info)),
        op::POW => format!(
            "vec4<f32>(pow(abs({}), {}))",
            scalar(&inst.src[0], info),
            scalar(&inst.src[1], info)
        ),
        op::EXP => format!("vec4<f32>(exp2({}))", scalar(&inst.src[0], info)),
        op::LOG => format!("vec4<f32>(log2(abs({})))", scalar(&inst.src[0], info)),
        op::DP3 => format!("vec4<f32>(dot(({}).xyz, ({}).xyz))", s(0), s(1)),
        op::DP4 => format!("vec4<f32>(dot({}, {}))", s(0), s(1)),
        op::DP2ADD => format!(
            "vec4<f32>(dot(({}).xy, ({}).xy) + {})",
            s(0),
            s(1),
            scalar(&inst.src[2], info)
        ),
        op::NRM => format!("vec4<f32>(normalize(({}).xyz), 0.0)", s(0)),
        _ => return Err(format!("unsupported opcode {o}")),
    };

    let dst = inst
        .dst
        .ok_or_else(|| format!("opcode {o} without destination"))?;
    write_masked(w, &dst, value, tmp, info);
    Ok(())
}

fn emit_texld(
    w: &mut String,
    inst: &Inst,
    info: &Info,
    depth: usize,
    tmp: &mut usize,
) -> Result<(), String> {
    let coord = inst.src.first().ok_or("texld without coord")?;
    let samp = inst.src.get(1).ok_or("texld without sampler")?;
    let si = samp.index;
    let dim = info
        .sampler_dim
        .get(&si)
        .copied()
        .unwrap_or(SamplerDim::Tex2D);
    let c = src(coord, info);
    let coord_sel = match dim {
        SamplerDim::Tex2D => format!("({c}).xy"),
        _ => format!("({c}).xyz"),
    };
    // textureSample needs uniform control flow; inside an if/else use an explicit LOD.
    let sample = if depth == 0 {
        format!("textureSample(t{si}, smp{si}, {coord_sel})")
    } else {
        format!("textureSampleLevel(t{si}, smp{si}, {coord_sel}, 0.0)")
    };
    // Linearise colour maps; data maps (normal/spec/mask) read raw.
    let value = if info.gamma_linearize && info.color_samplers.contains(&si) {
        format!("s2l4({sample})")
    } else {
        sample
    };
    let dst = inst.dst.ok_or("texld without destination")?;
    write_masked(w, &dst, value, tmp, info);
    Ok(())
}

fn cond_expr(inst: &Inst, _info: &Info) -> String {
    match inst.src.first() {
        Some(op) if op.kind == RegKind::ConstBool => {
            let neg = matches!(op.modifier, SrcMod::Neg);
            if neg {
                format!("!bget({}u)", op.index)
            } else {
                format!("bget({}u)", op.index)
            }
        }
        _ => "true".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modder::model::block_on;
    use ff13::formats::d3d9shader;

    #[test]
    fn emits_wgsl_for_minimal_shader() {
        let mut b: Vec<u8> = Vec::new();
        let mut dw = |v: u32| b.extend_from_slice(&v.to_le_bytes());
        dw(d3d9shader::PS_3_0);
        dw(81 | (5 << 24));
        dw(0x2000_0000);
        dw(1.0f32.to_bits());
        dw(2.0f32.to_bits());
        dw(3.0f32.to_bits());
        dw(4.0f32.to_bits());
        dw(1 | (2 << 24));
        dw(0x0000_0800 | (0xF << 16));
        dw(0x2000_0000 | (0xE4 << 16));
        dw(0x0000_FFFF);

        let sh = d3d9shader::decode(&b).unwrap();
        let t = transpile(&sh, &BTreeSet::new()).unwrap();
        assert!(t.wgsl.contains("fn fs_main"));
        assert!(t.wgsl.contains("pow(max(oC0.rgb"));
        assert!(t.wgsl.contains("vec4<f32>(1.0, 2.0, 3.0, 4.0)"));
    }

    #[test]
    fn real_shaders_transpile_to_valid_wgsl() {
        use eframe::egui_wgpu::wgpu;
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/chr/pc/c001/bin/c001.win32.trb");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let trb = ff13::formats::trb::Trb::parse(&bytes).unwrap();

        let instance = wgpu::Instance::default();
        let Some(adapter) =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("no wgpu adapter; skipping WGSL validation");
            return;
        };
        let (device, _q) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();

        let mut checked = 0;
        for i in 0..trb.resource_count() {
            let d = trb.resource_data(i).unwrap_or(&[]);
            if !d.starts_with(b"SEDBshd") {
                continue;
            }
            let Some(sh) = ff13::formats::sedbshd::main_pixel_shader(d) else {
                continue;
            };
            let t = match full_module(&sh, &BTreeSet::new(), None) {
                Ok(t) => t,
                Err(e) => panic!("transpile failed for material #{i}: {e}"),
            };
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("transpiled"),
                source: wgpu::ShaderSource::Wgsl(t.wgsl.clone().into()),
            });
            let err = block_on(device.pop_error_scope());
            assert!(
                err.is_none(),
                "material #{i} WGSL invalid: {err:?}\n{}",
                t.wgsl
            );
            checked += 1;
        }
        assert!(checked > 0, "no materials checked");
        eprintln!("validated transpiled WGSL for {checked} material(s)");
    }
}
