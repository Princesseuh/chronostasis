//! The HD-Models `.txt` edit script that builds a modded model from source models and bundles.

pub struct Op {
    pub tag: String,
    pub args: Vec<String>,
}

impl Op {
    /// The dispatch char (`V`, `~`, `P`, …): the tag without its leading colons or `/`.
    pub fn letter(&self) -> Option<char> {
        self.tag.trim_start_matches([':', '/']).chars().next()
    }

    fn render(&self) -> String {
        if self.args.is_empty() {
            self.tag.clone()
        } else {
            format!("{},{}", self.tag, self.args.join(","))
        }
    }
}

/// `:V,model,mesh,v1,v2,a5,a6,x,y,z`: edits every vertex in `[v1,v2]` of submesh `(model,mesh)`.
pub struct VertexOp {
    pub model: u16,
    pub mesh: u16,
    pub v1: u32,
    pub v2: u32,
    pub mode: u16,
    pub offset: usize,
    pub xyz: [f32; 3],
}

impl VertexOp {
    pub fn parse(op: &Op) -> Option<VertexOp> {
        if op.letter() != Some('V') || op.args.len() < 9 {
            return None;
        }
        let a = &op.args;
        Some(VertexOp {
            model: a[0].parse().ok()?,
            mesh: a[1].parse().ok()?,
            v1: a[2].parse().ok()?,
            v2: a[3].parse().ok()?,
            mode: a[4].parse().ok()?,
            offset: a[5].parse().ok()?,
            xyz: [a[6].parse().ok()?, a[7].parse().ok()?, a[8].parse().ok()?],
        })
    }

    /// `None` for the modes no shipped mod uses, which are unimplemented.
    pub fn edit(&self) -> Option<ff13_formats::wrb::VertexEdit> {
        use ff13_formats::wrb::VertexEdit::*;
        match self.mode {
            0 => Some(Position),
            3 => Some(Half2),
            4 => Some(Unorm2),
            _ => None,
        }
    }

    /// Returns how many vertices were written.
    pub fn apply_to_stms(&self, stms: &mut ff13_formats::wrb::Chunk) -> Option<usize> {
        let mode = self.edit()?;
        let (lo, hi) = (self.v1.min(self.v2), self.v1.max(self.v2));
        for v in lo..=hi {
            stms.apply_vertex_edit(v, self.offset, mode, self.xyz)?;
        }
        Some((hi - lo + 1) as usize)
    }
}

fn model_id(path: &str) -> Option<&str> {
    let base = path.rsplit(['\\', '/']).next()?;
    base.split('.').next().filter(|s| !s.is_empty())
}

pub struct Script {
    pub header: Vec<String>,
    pub ops: Vec<Op>,
}

impl Script {
    /// The header runs up to the first single-colon op. Double-colon `::A,…` prompt lines, which
    /// some costume scripts put in the header region, do NOT end it.
    pub fn parse(text: &str) -> Script {
        let mut header = Vec::new();
        let mut ops = Vec::new();
        let mut in_ops = false;
        for line in text.lines() {
            if !in_ops && line.starts_with(':') && !line.starts_with("::") {
                in_ops = true;
            }
            if in_ops {
                if line.is_empty() {
                    ops.push(Op {
                        tag: String::new(),
                        args: Vec::new(),
                    });
                    continue;
                }
                let (tag, rest) = line.split_once(',').unwrap_or((line, ""));
                let args = if line.contains(',') {
                    rest.split(',').map(str::to_string).collect()
                } else {
                    Vec::new()
                };
                ops.push(Op {
                    tag: tag.to_string(),
                    args,
                });
            } else {
                header.push(line.to_string());
            }
        }
        Script { header, ops }
    }

    /// In order, so an op's model index selects into this.
    pub fn model_paths(&self) -> Vec<&str> {
        // A model ref ends in `.win32`. A short label line like `pc\c001` also has a backslash
        // but is not a model, and matching it sends the combine loader at a nonexistent path.
        self.header
            .iter()
            .map(String::as_str)
            .filter(|l| l.ends_with(".win32"))
            .collect()
    }

    /// The outfit-swap assembly the combine pipeline consumes: output resource `i` is built from
    /// source `model`'s resource `resource`.
    pub fn combine_map(&self) -> Option<Vec<(usize, usize)>> {
        for (i, line) in self.header.iter().enumerate() {
            let Ok(n) = line.trim().parse::<usize>() else {
                continue;
            };
            let rest = &self.header[i + 1..];
            if n == 0 || rest.len() != n {
                continue;
            }
            let pairs: Option<Vec<(usize, usize)>> = rest
                .iter()
                .map(|l| {
                    let (a, b) = l.split_once(',')?;
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                })
                .collect();
            if let Some(p) = pairs {
                return Some(p);
            }
        }
        None
    }

    /// The header's bone-rename table (`hair`->`dmhr` etc.): count `n>=2` + `2n`
    /// plain name lines (`old`,`new`,…, no path or comma) where some key names a hair bone. This
    /// drives the skeleton-merge name fixup, which renames more bones than appear here.
    pub fn bone_renames(&self) -> Vec<(String, String)> {
        for (i, line) in self.header.iter().enumerate() {
            // The count may be negative, a flag used elsewhere; the pair list reads the same.
            let Ok(n) = line
                .trim()
                .parse::<i64>()
                .map(|v| v.unsigned_abs() as usize)
            else {
                continue;
            };
            if n < 2 {
                continue;
            }
            let Some(blk) = self.header.get(i + 1..i + 1 + 2 * n) else {
                continue;
            };
            let plain = blk
                .iter()
                .all(|x| !x.contains('\\') && !x.contains(',') && !x.trim().is_empty());
            if !plain {
                continue;
            }
            let pairs: Vec<(String, String)> = (0..n)
                .map(|j| {
                    (
                        blk[2 * j].trim().to_string(),
                        blk[2 * j + 1].trim().to_string(),
                    )
                })
                .collect();
            if pairs.iter().any(|(k, _)| k.contains("hair")) {
                return pairs;
            }
        }
        Vec::new()
    }

    /// A negative count means "add dmhr copies" and matches exact names only; a positive one
    /// renames source bones in place, matching by substring.
    pub fn bone_renames_add_mode(&self) -> bool {
        for (i, line) in self.header.iter().enumerate() {
            let Ok(v) = line.trim().parse::<i64>() else {
                continue;
            };
            let n = v.unsigned_abs() as usize;
            if n < 2 {
                continue;
            }
            let Some(blk) = self.header.get(i + 1..i + 1 + 2 * n) else {
                continue;
            };
            let plain = blk
                .iter()
                .all(|x| !x.contains('\\') && !x.contains(',') && !x.trim().is_empty());
            if plain && (0..n).any(|j| blk[2 * j].contains("hair")) {
                return v < 0;
            }
        }
        false
    }

    /// From the `::A,opts,default,question` header line; resolves `::I` blocks headlessly.
    pub fn interactive_default(&self) -> Option<String> {
        self.header
            .iter()
            .find(|l| l.starts_with("::A"))
            .and_then(|l| l.split(',').nth(2))
            .map(|s| s.trim().to_string())
    }

    /// From between the first `:X` pair, as `(bone index, field selector, value)`.
    pub fn skeleton_e_ops(&self) -> Vec<(usize, u8, i32)> {
        // `::E` and `:E` both give letter 'E', so `::`-tags have to be matched first.
        let default = self.interactive_default();
        let mut in_sec = false;
        let mut skip = false;
        let mut out = Vec::new();
        for op in &self.ops {
            if op.tag == "::I" {
                skip = op.args.get(1).map(|t| t.trim()) != default.as_deref();
                continue;
            }
            if op.tag == "::E" {
                skip = false;
                continue;
            }
            if op.letter() == Some('X') {
                if in_sec {
                    break;
                }
                in_sec = true;
                continue;
            }
            if skip {
                continue;
            }
            if in_sec && op.letter() == Some('E') && op.args.len() >= 3 {
                let a = op.args[0].trim().parse::<usize>();
                let b = op.args[1].trim().parse::<u8>();
                // Fields 6..=15 are floats and the rest ints, but store the raw 32-bit pattern
                // either way so a float op like `:E,211,13,0.0` survives an int-only parse.
                if let (Ok(a), Ok(b)) = (a, b) {
                    let raw = if (6..=15).contains(&b) {
                        op.args[2]
                            .trim()
                            .parse::<f32>()
                            .ok()
                            .map(|f| f.to_bits() as i32)
                    } else {
                        op.args[2].trim().parse::<i32>().ok()
                    };
                    if let Some(c) = raw {
                        out.push((a, b, c));
                    }
                }
            }
        }
        out
    }

    /// `:S` flattens the bone links into a chain.
    pub fn has_skeleton_s(&self) -> bool {
        let mut in_sec = false;
        for op in &self.ops {
            if op.letter() == Some('X') {
                if in_sec {
                    break;
                }
                in_sec = true;
                continue;
            }
            if in_sec && op.letter() == Some('S') {
                return true;
            }
        }
        false
    }

    /// `:F` "Face Fix" makes each facial bone take the target's own transform after the merge.
    pub fn has_skeleton_face_fix(&self) -> bool {
        let mut in_sec = false;
        for op in &self.ops {
            if op.letter() == Some('X') {
                if in_sec {
                    break;
                }
                in_sec = true;
                continue;
            }
            if in_sec && op.letter() == Some('F') {
                return true;
            }
        }
        false
    }

    pub fn has_skeleton_merge(&self) -> bool {
        let mut in_sec = false;
        for op in &self.ops {
            if op.letter() == Some('X') {
                if in_sec {
                    break;
                }
                in_sec = true;
                continue;
            }
            if in_sec && op.letter() == Some('O') {
                return true;
            }
        }
        false
    }

    /// Renames slot identity to the output id, and the rig the other way, since the rig follows
    /// the swapped-in mesh. Empty when there is no output id, or every source already matches.
    pub fn rename_pairs(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let Some(out) = self.output_paths().0.and_then(model_id) else {
            return Vec::new();
        };
        let mut pairs = Vec::new();
        for src in self.model_paths().into_iter().filter_map(model_id) {
            if src == out || src.len() < 2 || out.len() < 2 {
                continue;
            }
            pairs.push((
                format!("c_{src}").into_bytes(),
                format!("c_{out}").into_bytes(),
            ));
            // `cN0x_puls` rig naming is player-only; an NPC donor keeps its own physics names.
            if src.starts_with('c') {
                let (s2, o2) = (&src[1..2], &out[1..2]);
                pairs.push((
                    format!("c{o2}0x_puls").into_bytes(),
                    format!("c{s2}0x_puls").into_bytes(),
                ));
            }
        }
        pairs
    }

    pub fn output_paths(&self) -> (Option<&str>, Option<&str>) {
        let trb = self
            .header
            .iter()
            .find(|l| l.ends_with(".trb"))
            .map(String::as_str);
        let imgb = self
            .header
            .iter()
            .find(|l| l.ends_with(".imgb"))
            .map(String::as_str);
        (trb, imgb)
    }

    pub fn render(&self) -> String {
        let mut out: Vec<String> = self.header.clone();
        out.extend(self.ops.iter().map(Op::render));
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let text = "42\n0\n1\nchr\\pc\\c001\\bin\\c001.win32\n\
                    chr\\pc\\c201\\bin\\c201.win32.trb\n:V,0,0,47,47,0,0,0.08,0.5,0.1\n:X\n::A,2,1,Hide it?\n";
        let s = Script::parse(text);
        assert_eq!(s.ops.len(), 3);
        assert_eq!(s.ops[0].letter(), Some('V'));
        assert_eq!(
            s.ops[0].args,
            ["0", "0", "47", "47", "0", "0", "0.08", "0.5", "0.1"]
        );
        assert_eq!(s.ops[1].tag, ":X");
        assert_eq!(s.ops[2].letter(), Some('A'));
        assert_eq!(s.model_paths(), ["chr\\pc\\c001\\bin\\c001.win32"]);
        assert_eq!(
            s.output_paths().0,
            Some("chr\\pc\\c201\\bin\\c201.win32.trb")
        );
        assert_eq!(s.render(), text.trim_end_matches('\n'));
    }

    #[test]
    fn derives_rename_pairs() {
        let text = "42\n0\n2\n1\nc_c006\nc_c206\nc20x_pulsS\nc00x_pulsS\n\
                    chr\\pc\\c006\\bin\\c006.win32\nchr\\pc\\c206\\bin\\c206.win32\n\
                    chr\\pc\\c206\\bin\\c206.win32.trb\nchr\\pc\\c206\\bin\\c206.win32.imgb\n3\n0,0\n1,1\n0,2\n:X\n";
        let s = Script::parse(text);
        assert_eq!(
            s.rename_pairs(),
            vec![
                (b"c_c006".to_vec(), b"c_c206".to_vec()),
                (b"c20x_puls".to_vec(), b"c00x_puls".to_vec()),
            ]
        );
    }

    #[test]
    fn parses_combine_map() {
        let text = "42\n0\n2\n1\nc_c006\nc_c206\n\
                    chr\\pc\\c006\\bin\\c006.win32\nchr\\pc\\c206\\bin\\c206.win32\n\
                    chr\\pc\\c206\\bin\\c206.win32.trb\nchr\\pc\\c206\\bin\\c206.win32.imgb\n\
                    3\n0,0\n0,1\n1,2\n:O,1,0,1\n:X\n";
        let s = Script::parse(text);
        assert_eq!(s.combine_map(), Some(vec![(0, 0), (0, 1), (1, 2)]));
        let plain =
            Script::parse("42\n0\n1\nchr\\pc\\c001\\bin\\c001.win32\n:V,0,0,0,0,0,0,0.0,0.0,0.0\n");
        assert_eq!(plain.combine_map(), None);
    }

    #[test]
    fn parses_bone_renames_and_skeleton_section() {
        let text = "42\n0\n1\n1\nc_c001\nc_c201\n\
                    2\nL_hair1a\nL_dmhr1a\nR_hair3a\nR_dmhr3a\n\
                    2\nchr\\pc\\c001\\bin\\c001.win32\nchr\\pc\\c201\\bin\\c201.win32\n\
                    chr\\pc\\c201\\bin\\c201.win32.trb\nchr\\pc\\c201\\bin\\c201.win32.imgb\n\
                    1\n0,0\n:~,1,1,0,0.0,0.0,X.txt,b.bin\n:X\n:O,1,0,1\n:E,4,4,9\n:E,79,4,-1\n:X\n";
        let s = Script::parse(text);
        assert_eq!(
            s.bone_renames(),
            vec![
                ("L_hair1a".to_string(), "L_dmhr1a".to_string()),
                ("R_hair3a".to_string(), "R_dmhr3a".to_string()),
            ]
        );
        assert!(s.has_skeleton_merge());
        assert_eq!(s.skeleton_e_ops(), vec![(4, 4, 9), (79, 4, -1)]);
        let plain = Script::parse(":V,0,0,0,0,0,0,0.0,0.0,0.0\n");
        assert!(!plain.has_skeleton_merge());
        assert!(plain.skeleton_e_ops().is_empty());
    }

    #[test]
    fn header_survives_interactive_prompt() {
        // A `::A` prompt before the model data must not end the header.
        let text = "50\n0\n::A,2,1,Hide it?\n1\n1\nc_n906\nc_c201\n\
                    2\nB_hair1a\nB_dmhr1a\nL_hair1a\nL_dmhr1a\n\
                    2\nchr\\pc\\n906\\bin\\n906.win32\nchr\\pc\\c201\\bin\\c201.win32\n\
                    chr\\pc\\c201\\bin\\c201.win32.trb\nchr\\pc\\c201\\bin\\c201.win32.imgb\n\
                    2\n0,0\n1,1\n:~,1,0,0,0.0,0.0,X.txt,b.bin\n";
        let s = Script::parse(text);
        assert_eq!(
            s.output_paths().0,
            Some("chr\\pc\\c201\\bin\\c201.win32.trb")
        );
        assert_eq!(s.model_paths().len(), 2);
        assert_eq!(s.combine_map(), Some(vec![(0, 0), (1, 1)]));
        assert_eq!(
            s.bone_renames(),
            vec![
                ("B_hair1a".to_string(), "B_dmhr1a".to_string()),
                ("L_hair1a".to_string(), "L_dmhr1a".to_string()),
            ]
        );
    }

    #[test]
    fn parses_vertex_op() {
        let s = Script::parse(":V,1,5,300,305,0,0,0.5,0.5,0.9\n");
        let v = VertexOp::parse(&s.ops[0]).unwrap();
        assert_eq!(
            (v.model, v.mesh, v.v1, v.v2, v.mode, v.offset),
            (1, 5, 300, 305, 0, 0)
        );
        assert_eq!(v.xyz, [0.5, 0.5, 0.9]);
        assert_eq!(v.edit(), Some(ff13_formats::wrb::VertexEdit::Position));
        let s2 = Script::parse(":V,0,0,0,0,9,0,0.0,0.0,0.0\n");
        assert_eq!(VertexOp::parse(&s2.ops[0]).unwrap().edit(), None);
    }

    #[test]
    fn applies_vertex_op_to_model() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        use ff13_formats::wrb;
        let mut found = false;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        'outer: while let Some(d) = stack.pop() {
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
                let trb = std::fs::read(&p).unwrap();
                let Some(res) = first_wrb_resource(&trb) else {
                    continue;
                };
                let mut root = wrb::parse(&res).unwrap();
                let op =
                    VertexOp::parse(&Script::parse(":V,0,0,0,0,0,0,0.9,0.9,0.9\n").ops[0]).unwrap();
                if apply_to_first_position_stms(&mut root, &op) {
                    let raw = first_position_raw(&root).unwrap();
                    assert_eq!(
                        raw,
                        [29490, 29490, 29490],
                        "VertexOp::apply in {}",
                        p.display()
                    );
                    found = true;
                    break 'outer;
                }
            }
        }
        assert!(found, "no position stream found to edit");
    }

    fn first_wrb_resource(trb: &[u8]) -> Option<Vec<u8>> {
        use byteorder::{ByteOrder, LittleEndian as LE};
        if trb.len() < 64 || &trb[..8] != b"SEDBRES " {
            return None;
        }
        let rc = LE::read_u32(&trb[56..60]) as usize;
        let ds = 64 + rc * 16;
        for i in 0..rc {
            let e = 64 + i * 16;
            let off = LE::read_u32(&trb[e + 4..e + 8]) as usize;
            let sz = LE::read_u32(&trb[e + 8..e + 12]) as usize;
            let d = trb.get(ds + off..ds + off + sz)?;
            if d.len() > 7 && &d[..7] == b"SEDBwrb" {
                return Some(d.to_vec());
            }
        }
        None
    }

    fn apply_to_first_position_stms(c: &mut ff13_formats::wrb::Chunk, op: &VertexOp) -> bool {
        if c.as_stms()
            .map(|s| s.position_offset().is_some())
            .unwrap_or(false)
        {
            return op.apply_to_stms(c).is_some();
        }
        c.children_mut()
            .iter_mut()
            .any(|k| apply_to_first_position_stms(k, op))
    }

    fn first_position_raw(c: &ff13_formats::wrb::Chunk) -> Option<[i16; 3]> {
        if let Some(s) = c.as_stms()
            && s.position_offset().is_some()
        {
            return s.position_raw(0);
        }
        c.children().iter().find_map(first_position_raw)
    }

    #[test]
    fn round_trips_shipped_scripts() {
        let Ok(dir) = std::env::var("FF13_MODELSCRIPTS_DIR") else {
            return;
        };
        let mut n = 0;
        for ent in std::fs::read_dir(dir).unwrap().flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            let normalized = text.replace("\r\n", "\n");
            let s = Script::parse(&normalized);
            assert_eq!(
                s.render(),
                normalized.trim_end_matches('\n'),
                "round-trip {}",
                p.display()
            );
            n += 1;
        }
        eprintln!("round-tripped {n} scripts");
        assert!(n > 0);
    }

    #[test]
    fn model_paths_ignores_label_line() {
        // The label line has a backslash but is not a model.
        let text = "42\n32\npc\\c001\n0\n0\n0\n1\n\
                    chr\\pc\\c001\\bin\\c001.win32\n\
                    chr\\pc\\c001\\bin\\c001.win32.trb\nchr\\pc\\c001\\bin\\c001.win32.imgb\n\
                    1\n0,0\n:X\n";
        let s = Script::parse(text);
        assert_eq!(s.model_paths(), vec!["chr\\pc\\c001\\bin\\c001.win32"]);
    }

    #[test]
    fn model_paths_keeps_all_combine_sources() {
        let text = "42\n0\n2\n1\nc_c006\nc_c206\n\
                    chr\\pc\\c006\\bin\\c006.win32\nchr\\pc\\c206\\bin\\c206.win32\n\
                    chr\\pc\\c206\\bin\\c206.win32.trb\nchr\\pc\\c206\\bin\\c206.win32.imgb\n\
                    3\n0,0\n0,1\n1,2\n:X\n";
        let s = Script::parse(text);
        assert_eq!(
            s.model_paths(),
            vec![
                "chr\\pc\\c006\\bin\\c006.win32",
                "chr\\pc\\c206\\bin\\c206.win32"
            ]
        );
    }
}
