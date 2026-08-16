//! `SEDBSKL` skeleton resource: the joint hierarchy the mesh's `ENVD` bone palettes deform.

use byteorder::{ByteOrder, LittleEndian as LE};

const JOINT_SIZE: usize = 0xB0;

/// One joint of a [`Skeleton`]; every transform on it is parent-relative.
#[derive(Debug, Clone)]
pub struct Joint {
    pub name: String,
    /// `"NullNode"` is a helper transform, `"JointNode"` a deforming bone.
    pub kind: String,
    pub translation: [f32; 3],
    /// `(x, y, z, w)`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// `-1` for a root.
    pub parent: i32,
    /// `-1` on a leaf. The rest of the children follow that joint's
    /// [`next_sibling`](Joint::next_sibling) chain, which [`Skeleton::children`] walks.
    pub first_child: i32,
    /// `-1` at the end of the chain.
    pub next_sibling: i32,
    /// What animations reference this joint by.
    pub hash: u32,
    /// A `MINS` model instance on a `MODEL` joint, a scene light on a `LightNode`, else `-1`.
    pub object_index: i32,
    /// Meaning unconfirmed; only `0x02` and `0x12` occur in FFXIII.
    pub bitfield: u32,
    pub group_id: u32,
}

/// Joints in file order; every joint index on a [`Joint`] indexes this list.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}

impl Skeleton {
    /// `res` starts at the `SEDB` section header.
    pub fn parse(section: &[u8]) -> Option<Skeleton> {
        if section.len() < 0x40 || &section[0..4] != b"SEDB" || &section[4..7] != b"SKL" {
            return None;
        }
        let u32 = |o: usize| -> Option<u32> { section.get(o..o + 4).map(LE::read_u32) };
        let i32 = |o: usize| -> Option<i32> { section.get(o..o + 4).map(LE::read_i32) };
        let f32 = |o: usize| -> Option<f32> { section.get(o..o + 4).map(LE::read_f32) };

        let n_sub = u32(0x30)? as usize;
        let name_table_off = u32(0x34)? as usize;
        let name_count = u32(0x38)? as usize;
        let data = 0x40 + n_sub.checked_mul(0x10)?;

        let names = read_string_pool(section, data.checked_add(name_table_off)?, name_count);
        let name = |idx: usize| names.get(idx).cloned().unwrap_or_default();

        let mut joints = Vec::new();
        for s in 0..n_sub {
            let h = 0x40 + s * 0x10;
            let start = data + u32(h + 4)? as usize;
            let end = data + u32(h + 8)? as usize;
            if end <= start || !(end - start).is_multiple_of(JOINT_SIZE) || end > section.len() {
                continue;
            }
            for j in (start..end).step_by(JOINT_SIZE) {
                joints.push(Joint {
                    name: name(u32(j)? as usize),
                    kind: name(u32(j + 4)? as usize),
                    translation: [f32(j + 0x10)?, f32(j + 0x14)?, f32(j + 0x18)?],
                    rotation: [
                        f32(j + 0x1c)?,
                        f32(j + 0x20)?,
                        f32(j + 0x24)?,
                        f32(j + 0x28)?,
                    ],
                    scale: [f32(j + 0x2c)?, f32(j + 0x30)?, f32(j + 0x34)?],
                    parent: i32(j + 0x38)?,
                    first_child: i32(j + 0x3c)?,
                    next_sibling: i32(j + 0x40)?,
                    hash: u32(j + 0x44)?,
                    object_index: i32(j + 0x0c)?,
                    bitfield: u32(j + 0x48)?,
                    group_id: u32(j + 0x4c)?,
                });
            }
        }
        Some(Skeleton { joints })
    }
}

impl Joint {
    /// Column-major to match `glam::Mat4::from_cols_array_2d`.
    pub fn local_matrix(&self) -> [[f32; 4]; 4] {
        let [x, y, z, w] = self.rotation;
        let n = (x * x + y * y + z * z + w * w).sqrt();
        let (x, y, z, w) = if n > 0.0 {
            (x / n, y / n, z / n, w / n)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        };
        let s = self.scale;
        let t = self.translation;
        [
            [
                (1.0 - 2.0 * (y * y + z * z)) * s[0],
                2.0 * (x * y + z * w) * s[0],
                2.0 * (x * z - y * w) * s[0],
                0.0,
            ],
            [
                2.0 * (x * y - z * w) * s[1],
                (1.0 - 2.0 * (x * x + z * z)) * s[1],
                2.0 * (y * z + x * w) * s[1],
                0.0,
            ],
            [
                2.0 * (x * z + y * w) * s[2],
                2.0 * (y * z - x * w) * s[2],
                (1.0 - 2.0 * (x * x + y * y)) * s[2],
                0.0,
            ],
            [t[0], t[1], t[2], 1.0],
        ]
    }
}

impl Skeleton {
    /// Ascending index order. Equivalent to filtering on [`Joint::parent`], and cheaper.
    pub fn children(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        let mut next = self.joints.get(index).map_or(-1, |j| j.first_child);
        let mut guard = 0;
        std::iter::from_fn(move || {
            let cur = usize::try_from(next)
                .ok()
                .filter(|_| guard < self.joints.len())?;
            let j = self.joints.get(cur)?;
            next = j.next_sibling;
            guard += 1;
            Some(cur)
        })
    }

    /// Parents are not guaranteed to precede their children, so each joint walks to the root.
    pub fn bind_world(&self) -> Vec<[[f32; 4]; 4]> {
        let locals: Vec<_> = self.joints.iter().map(Joint::local_matrix).collect();
        (0..self.joints.len())
            .map(|i| {
                let mut m = locals[i];
                let mut p = self.joints[i].parent;
                let mut guard = 0;
                while p >= 0 && (p as usize) < self.joints.len() && guard < self.joints.len() {
                    m = mat4_mul(&locals[p as usize], &m);
                    p = self.joints[p as usize].parent;
                    guard += 1;
                }
                m
            })
            .collect()
    }

    /// The inverse-bind that maps a bind-pose vertex into joint space; singular joints fall back to identity.
    pub fn inverse_bind(&self) -> Vec<[[f32; 4]; 4]> {
        self.bind_world()
            .iter()
            .map(|m| invert_affine(m).unwrap_or(IDENTITY))
            .collect()
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Applies `b` then `a`.
fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
        }
    }
    o
}

/// Affine only (last row `0 0 0 1`). `None` if the linear part is singular.
fn invert_affine(m: &[[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    // Transposed to row-major here, then transposed back at the end.
    let l = [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ];
    let det = l[0][0] * (l[1][1] * l[2][2] - l[1][2] * l[2][1])
        - l[0][1] * (l[1][0] * l[2][2] - l[1][2] * l[2][0])
        + l[0][2] * (l[1][0] * l[2][1] - l[1][1] * l[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let d = 1.0 / det;
    let inv = [
        [
            (l[1][1] * l[2][2] - l[1][2] * l[2][1]) * d,
            (l[0][2] * l[2][1] - l[0][1] * l[2][2]) * d,
            (l[0][1] * l[1][2] - l[0][2] * l[1][1]) * d,
        ],
        [
            (l[1][2] * l[2][0] - l[1][0] * l[2][2]) * d,
            (l[0][0] * l[2][2] - l[0][2] * l[2][0]) * d,
            (l[0][2] * l[1][0] - l[0][0] * l[1][2]) * d,
        ],
        [
            (l[1][0] * l[2][1] - l[1][1] * l[2][0]) * d,
            (l[0][1] * l[2][0] - l[0][0] * l[2][1]) * d,
            (l[0][0] * l[1][1] - l[0][1] * l[1][0]) * d,
        ],
    ];
    let t = [m[3][0], m[3][1], m[3][2]];
    let it = [
        -(inv[0][0] * t[0] + inv[0][1] * t[1] + inv[0][2] * t[2]),
        -(inv[1][0] * t[0] + inv[1][1] * t[1] + inv[1][2] * t[2]),
        -(inv[2][0] * t[0] + inv[2][1] * t[1] + inv[2][2] * t[2]),
    ];
    Some([
        [inv[0][0], inv[1][0], inv[2][0], 0.0],
        [inv[0][1], inv[1][1], inv[2][1], 0.0],
        [inv[0][2], inv[1][2], inv[2][2], 0.0],
        [it[0], it[1], it[2], 1.0],
    ])
}

fn read_string_pool(data: &[u8], start: usize, count: usize) -> Vec<String> {
    // `count` is untrusted, so cap the capacity by the bytes actually present.
    let mut out = Vec::with_capacity(count.min(data.len().saturating_sub(start)));
    let mut o = start;
    while out.len() < count && o < data.len() {
        let s = o;
        while o < data.len() && data[o] != 0 {
            o += 1;
        }
        out.push(String::from_utf8_lossy(&data[s..o]).into_owned());
        o += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Joint, Skeleton};
    use crate::trb::Trb;

    fn joint(name: &str, parent: i32, t: [f32; 3], rot: [f32; 4]) -> Joint {
        Joint {
            name: name.into(),
            kind: "JointNode".into(),
            translation: t,
            rotation: rot,
            scale: [1.0, 1.0, 1.0],
            parent,
            first_child: -1,
            next_sibling: -1,
            hash: 0,
            object_index: -1,
            bitfield: 0,
            group_id: 0,
        }
    }

    #[test]
    fn bind_world_and_inverse() {
        let s = (0.5f32).sqrt();
        let skel = Skeleton {
            joints: vec![
                joint("root", -1, [1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
                joint("child", 0, [0.0, 2.0, 0.0], [0.0, 0.0, s, s]),
            ],
        };
        let world = skel.bind_world();
        let pos = |m: &[[f32; 4]; 4]| [m[3][0], m[3][1], m[3][2]];
        assert!(dist(pos(&world[0]), [1.0, 0.0, 0.0]) < 1e-5);
        assert!(dist(pos(&world[1]), [1.0, 2.0, 0.0]) < 1e-5);

        let inv = skel.inverse_bind();
        let mul = |a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]| {
            let mut o = [[0.0f32; 4]; 4];
            for c in 0..4 {
                for r in 0..4 {
                    for k in 0..4 {
                        o[c][r] += a[k][r] * b[c][k];
                    }
                }
            }
            o
        };
        for i in 0..skel.joints.len() {
            let p = mul(&world[i], &inv[i]);
            for (c, col) in p.iter().enumerate() {
                for (r, &v) in col.iter().enumerate() {
                    let want = if c == r { 1.0 } else { 0.0 };
                    assert!(
                        (v - want).abs() < 1e-4,
                        "joint {i}: world*inverse not identity"
                    );
                }
            }
        }
    }

    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn skeletons_parse() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (mut skeletons, mut joints) = (0u64, 0u64);
        let mut stack = vec![std::path::PathBuf::from(dir)];
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
                let Ok(trb) = Trb::parse(&bytes) else {
                    continue;
                };
                let Some(skel) = trb.skeleton() else { continue };
                skeletons += 1;
                let n = skel.joints.len() as i32;
                assert!(n > 0, "empty skeleton in {}", p.display());
                let mut roots = 0;
                for (i, j) in skel.joints.iter().enumerate() {
                    joints += 1;
                    assert!(
                        j.parent >= -1 && j.parent < n && j.parent != i as i32,
                        "joint {i} bad parent {} (n={n}) in {}",
                        j.parent,
                        p.display()
                    );
                    if j.parent == -1 {
                        roots += 1;
                    }
                    for c in [j.first_child, j.next_sibling] {
                        assert!(
                            c >= -1 && c < n,
                            "joint {i} bad link {c} in {}",
                            p.display()
                        );
                    }
                    let q = j.rotation;
                    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                    assert!(
                        (0.5..=1.5).contains(&len),
                        "joint {i} rotation not ~unit quaternion ({len}) in {}",
                        p.display()
                    );
                }
                assert!(roots >= 1, "no root joint in {}", p.display());
                assert!(
                    !skel.joints[0].name.is_empty(),
                    "unnamed root in {}",
                    p.display()
                );

                for i in 0..skel.joints.len() {
                    let by_parent: Vec<usize> = (0..skel.joints.len())
                        .filter(|&k| skel.joints[k].parent == i as i32)
                        .collect();
                    let by_link: Vec<usize> = skel.children(i).collect();
                    assert_eq!(
                        by_link,
                        by_parent,
                        "joint {i} child links disagree with the parent indices in {}",
                        p.display()
                    );
                }
            }
        }
        eprintln!("parsed {skeletons} skeletons, {joints} joints");
        assert!(
            skeletons > 0,
            "no SEDBSKL skeletons found under FF13_MODELS_DIR"
        );
    }

    #[test]
    fn children_survives_a_cycle() {
        let mut skel = Skeleton {
            joints: vec![
                joint("root", -1, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                joint("a", 0, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                joint("b", 0, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
            ],
        };
        skel.joints[0].first_child = 1;
        skel.joints[1].next_sibling = 2;
        assert_eq!(skel.children(0).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(skel.children(1).count(), 0);

        skel.joints[2].next_sibling = 1;
        assert_eq!(skel.children(0).count(), skel.joints.len());
    }
}
