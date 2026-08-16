//! `SEDBPHB` physics: the collision volumes and rigs driving a model's cloth and hair.

use byteorder::{ByteOrder, LittleEndian as LE};

const SEDB_HEADER: usize = 48;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Sphere,
    Plane,
    Capsule,
    Unknown(u32),
}

impl Shape {
    fn from_raw(v: u32) -> Shape {
        match v {
            1 => Shape::Sphere,
            2 => Shape::Plane,
            3 => Shape::Capsule,
            other => Shape::Unknown(other),
        }
    }
}

/// One collision volume, positioned in the local space of the joint it names.
#[derive(Clone, Debug)]
pub struct Volume {
    pub shape: Shape,
    pub shape_name: String,
    /// Empty when the record names no joint.
    pub joint: String,
    pub name: String,
    pub offset: [f32; 3],
    pub rotation: [f32; 3],
    pub radius: f32,
    /// Meaning varies by shape (capsule length, plane extent, ...).
    pub extra: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub volumes: Vec<Volume>,
}

/// An entry whose type we recognise but do not decode.
#[derive(Clone, Copy, Debug)]
pub struct RawEntry {
    pub entry_type: u32,
    pub offset: usize,
}

/// One simulated joint of a [`Chain`]; the tuning floats' roles are not identified.
#[derive(Clone, Debug)]
pub struct ChainJoint {
    pub joint: String,
    pub params: [f32; 3],
}

/// One chain of simulated joints (hair strand, cape edge, ...) inside a [`Rig`].
#[derive(Clone, Debug)]
pub struct Chain {
    pub name: String,
    /// Joint the chain hangs from.
    pub joint: String,
    /// Roles not identified.
    pub params: [f32; 8],
    pub joints: Vec<ChainJoint>,
    /// Scoped to THIS strand; widening it lets a weapon volume sweep a character's hair away.
    pub colliders: Vec<Constraint>,
}

/// Kinds 1/3/5 are volumes and 7/8 planes; the individual float roles are not identified.
#[derive(Clone, Debug)]
pub struct Constraint {
    pub kind: u32,
    pub joints: Vec<String>,
    pub params: [f32; 6],
    pub extra_floats: Vec<f32>,
    pub extra_ints: Vec<u32>,
}

/// `group` indexes the rig's chains followed by its parts; `joint` indexes that group's own list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attach {
    pub group: usize,
    pub joint: usize,
}

/// A constraint between simulated joints, addressed by index rather than by name.
#[derive(Clone, Debug)]
pub enum Spring {
    Pin {
        name: String,
        at: Attach,
        anchor: String,
    },
    /// `stiffness` is 0..1.
    Link {
        name: String,
        a: Attach,
        b: Attach,
        rest: f32,
        stiffness: f32,
    },
    Nail {
        name: String,
        at: Attach,
        anchor: String,
        offset: [f32; 3],
        strength: f32,
    },
}

impl Spring {
    pub fn name(&self) -> &str {
        match self {
            Spring::Pin { name, .. } | Spring::Link { name, .. } | Spring::Nail { name, .. } => {
                name
            }
        }
    }
}

/// Per-joint gravity and wind: a unit direction in `reference`'s space, scaled by `magnitude`.
#[derive(Clone, Debug)]
pub struct Force {
    pub name: String,
    pub reference: String,
    pub direction: [f32; 3],
    pub magnitude: f32,
}

/// One particle of a [`Part`]. `TRBLib` has the two sub-lists swapped, so it sees no constraints here.
#[derive(Clone, Debug)]
pub struct PartNode {
    pub joint: String,
    pub params: [f32; 3],
    pub params2: [f32; 3],
    /// Rest orientation and direction, both unit length.
    pub rest: Option<([f32; 4], [f32; 3])>,
    /// Scoped to THIS node; widening to the whole part puts particles through foreign planes.
    pub colliders: Vec<Constraint>,
    pub forces: Vec<Force>,
}

#[derive(Clone, Copy, Debug)]
pub struct PartLink {
    pub a: usize,
    pub b: usize,
    pub kind: u32,
    pub rest: f32,
}

/// A cloth mesh rather than a single strand. The leading `structural` links run along the mesh and
/// the rest cross it; `shear` holds each grid cell's diagonals.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub nodes: Vec<PartNode>,
    pub links: Vec<PartLink>,
    pub structural: usize,
    pub shear: Vec<(i16, i16)>,
    pub params: [f32; 8],
    pub colliders: Vec<Constraint>,
}

/// A type-4 entry: the simulation rig for one model.
#[derive(Clone, Debug, Default)]
pub struct Rig {
    pub name: String,
    pub secondary_name: String,
    pub chains: Vec<Chain>,
    pub constraints: Vec<Constraint>,
    pub parts: Vec<Part>,
    pub springs: Vec<Spring>,
}

/// What an [`IkNode`] solves for. The file stores these as `0x200 | kind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IkKind {
    /// `0x201`. Upper arm and forearm.
    Arm,
    /// `0x202`. Head look-at; its [`IkKind::LookAtJoint`] children spread the turn down neck and spine.
    LookAt,
    /// `0x203`. One joint's share of the parent [`IkKind::LookAt`].
    LookAtJoint,
    /// `0x204`. Aims one joint at another, used for the eyes.
    Aim,
    /// `0x205`. An eyelid joint following the parent [`IkKind::Aim`].
    AimFollow,
    /// `0x206`. Two-bone IK over a root/mid/end chain: legs, arms, quadruped limbs.
    TwoBone,
    /// `0x209`. Places a single joint, mostly toes and fingers.
    JointPlace,
    /// `0x20a`. Hip height adjust; parents the leg [`IkKind::TwoBone`] solvers it must keep reachable.
    HipAdjust,
    /// `0x20b`. Hip rotation; also parents the leg [`IkKind::TwoBone`] solvers.
    HipRotate,
    Unknown(u16),
}

/// A `(min, max)` rotation limit, in DEGREES.
#[derive(Clone, Copy, Debug)]
pub struct AngleLimit {
    pub min: f32,
    pub max: f32,
}

/// The ground-contact geometry an [`IkKind::TwoBone`] solver needs to stand a foot on a slope.
#[derive(Clone, Copy, Debug)]
pub struct FootPlant {
    /// Bind-pose height of the end joint above the floor, so the solver targets `ground + height`.
    pub height: f32,
    /// Never below [`height`](FootPlant::height). Role unidentified.
    pub raised_height: f32,
    /// The world up axis in the end joint's bind-pose frame.
    pub up: [f32; 3],
}

/// One solver of an [`IkRig`], plus the solvers nested under it.
#[derive(Clone, Debug)]
pub struct IkNode {
    pub kind: IkKind,
    pub name: String,
    /// Root first, with the blank slots the file leaves dropped.
    pub joints: Vec<String>,
    /// In degrees, in file order.
    pub limits: Vec<AngleLimit>,
    /// Only [`IkKind::TwoBone`] carries this.
    pub plant: Option<FootPlant>,
    /// Remaining floats in file order; roles unidentified.
    pub params: Vec<f32>,
    /// Remaining integer words in file order; roles unidentified.
    pub flags: Vec<u32>,
    pub children: Vec<IkNode>,
}

impl IkNode {
    /// Parents before children.
    pub fn walk(&self) -> Vec<&IkNode> {
        let mut out = vec![self];
        let mut i = 0;
        while i < out.len() {
            out.extend(out[i].children.iter());
            i += 1;
        }
        out
    }
}

/// A type-7 entry: one named group of procedural-animation solvers. Names are Japanese shorthand
/// for what the group drives, e.g. `k_kubi` (neck), `i_2j` (2-joint).
#[derive(Clone, Debug)]
pub struct IkRig {
    pub name: String,
    /// Passed to the sub-records and never read again, so it selects no layout.
    pub flag: u16,
    pub nodes: Vec<IkNode>,
}

#[derive(Default, Debug)]
pub struct Phb {
    pub groups: Vec<Group>,
    /// The capsule the world sweeps this model against, as opposed to the per-joint volumes in
    /// [`groups`](Phb::groups) that cloth and hair collide with.
    pub world_collision: Vec<Group>,
    pub rigs: Vec<Rig>,
    pub ik_rigs: Vec<IkRig>,
    pub undecoded: Vec<RawEntry>,
}

impl Phb {
    /// Excludes world-collision capsules; read [`world_collision`](Phb::world_collision) for those.
    pub fn volumes(&self) -> impl Iterator<Item = &Volume> {
        self.groups.iter().flat_map(|g| g.volumes.iter())
    }

    pub fn ik_nodes(&self) -> impl Iterator<Item = &IkNode> {
        self.ik_rigs
            .iter()
            .flat_map(|r| r.nodes.iter().flat_map(IkNode::walk))
    }

    pub fn parse(res: &[u8]) -> Option<Phb> {
        if !res.starts_with(b"SEDBPHB") {
            return None;
        }
        let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
        let table = SEDB_HEADER + 24;
        let count = u32at(SEDB_HEADER + 16)? as usize;
        if count == 0 || count > 1000 || table + count * 4 > res.len() {
            return Some(Phb::default());
        }
        let mut out = Phb::default();
        for k in 0..count {
            // Each slot's offset is relative to the slot itself, not to the table start.
            let at = table + u32at(table + k * 4)? as usize + k * 4;
            if at + 4 > res.len() {
                continue;
            }
            let entry_type = u32at(at)?;
            match entry_type {
                // Type 1 differs from type 2 only in a trailing descriptor we do not need.
                1 | 2 => match (read_group(res, at), entry_type) {
                    (Some(g), 1) => out.world_collision.push(g),
                    (Some(g), _) => out.groups.push(g),
                    (None, t) => out.undecoded.push(RawEntry {
                        entry_type: t,
                        offset: at,
                    }),
                },
                4 => match read_rig(res, at) {
                    Some(r) => out.rigs.push(r),
                    None => out.undecoded.push(RawEntry {
                        entry_type: 4,
                        offset: at,
                    }),
                },
                7 => match read_ik_rig(res, at) {
                    Some(r) => out.ik_rigs.push(r),
                    None => out.undecoded.push(RawEntry {
                        entry_type: 7,
                        offset: at,
                    }),
                },
                t => out.undecoded.push(RawEntry {
                    entry_type: t,
                    offset: at,
                }),
            }
        }
        Some(out)
    }
}

fn read_group(res: &[u8], at: usize) -> Option<Group> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let u64at = |o: usize| res.get(o..o + 8).map(LE::read_u64);
    let name_rel = u64at(at + 4)? as usize;
    let array_rel = u32at(at + 24)? as usize;
    let vol_count = u32at(at + 28)? as usize;
    let name = cstr(res, at + 4 + name_rel).unwrap_or_default();
    let mut volumes = Vec::new();
    if vol_count <= 100 {
        let base = at + 24 + array_rel;
        for i in 0..vol_count {
            let slot = base + i * 12;
            if slot + 12 > res.len() {
                break;
            }
            if let Some(v) = read_volume(res, slot) {
                volumes.push(v);
            }
        }
    }
    Some(Group { name, volumes })
}

fn read_volume(res: &[u8], slot: usize) -> Option<Volume> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let u64at = |o: usize| res.get(o..o + 8).map(LE::read_u64);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let rec = slot + u32at(slot)? as usize;
    if rec + 4 > res.len() {
        return None;
    }
    let shape = Shape::from_raw(u32at(rec)?);
    let shape_name = rel_cstr(res, rec + 4).unwrap_or_default();
    let joint = rel_cstr(res, rec + 8).unwrap_or_default();
    let body = rec + 12;
    if body + 48 > res.len() {
        return None;
    }
    let v3 = |o: usize| -> Option<[f32; 3]> { Some([f32at(o)?, f32at(o + 4)?, f32at(o + 8)?]) };
    let offset = v3(body + 4)?;
    let rotation = v3(body + 16)?;
    let radius = f32at(body + 28)?;
    let extra = [
        f32at(body + 32)?,
        f32at(body + 36)?,
        f32at(body + 40)?,
        f32at(body + 44)?,
    ];
    let name_rel = u64at(slot + 4)? as usize;
    let name = cstr(res, slot + 4 + name_rel).unwrap_or_default();
    Some(Volume {
        shape,
        shape_name,
        joint,
        name,
        offset,
        rotation,
        radius,
        extra,
    })
}

/// Each sub-list descriptor's offset is relative to its own slot (+24, +32, +40, +52 from the entry).
fn read_rig(res: &[u8], at: usize) -> Option<Rig> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let mut rig = Rig {
        name: rel_cstr(res, at + 4).unwrap_or_default(),
        secondary_name: rel_cstr(res, at + 8).unwrap_or_default(),
        ..Rig::default()
    };
    let chain_count = u32at(at + 28)? as usize;
    if chain_count <= 1000 {
        let base = at + 24 + u32at(at + 24)? as usize;
        for i in 0..chain_count {
            let o = base + i * 68;
            if o + 68 > res.len() {
                break;
            }
            let params = (0..8)
                .map(|k| res.get(o + 24 + k * 4..o + 28 + k * 4).map(LE::read_f32))
                .collect::<Option<Vec<f32>>>()?;
            let mut joints = Vec::new();
            let seg_count = u32at(o + 12)? as usize;
            if seg_count <= 1000 {
                let base = o + 8 + u32at(o + 8)? as usize;
                for k in 0..seg_count {
                    let q = base + k * 36;
                    if q + 36 > res.len() {
                        break;
                    }
                    let jp = (0..3)
                        .map(|c| res.get(q + 20 + c * 4..q + 24 + c * 4).map(LE::read_f32))
                        .collect::<Option<Vec<f32>>>()?;
                    joints.push(ChainJoint {
                        joint: rel_cstr(res, q).unwrap_or_default(),
                        params: jp.try_into().ok()?,
                    });
                }
            }
            let mut colliders = Vec::new();
            let col_count = u32at(o + 20)? as usize;
            if col_count <= 1000 {
                let table = o + 16 + u32at(o + 16)? as usize;
                for k in 0..col_count {
                    let slot = table + k * 4;
                    let Some(rel) = u32at(slot) else { break };
                    if let Some((c, _)) = read_constraint(res, slot + rel as usize) {
                        colliders.push(c);
                    }
                }
            }
            rig.chains.push(Chain {
                name: rel_cstr(res, o).unwrap_or_default(),
                joint: rel_cstr(res, o + 4).unwrap_or_default(),
                params: params.try_into().ok()?,
                joints,
                colliders,
            });
        }
    }
    let part_count = u32at(at + 56)? as usize;
    if part_count <= 1000 {
        let base = at + 52 + u32at(at + 52)? as usize;
        for k in 0..part_count {
            let p = base + k * 112;
            if p + 112 > res.len() {
                break;
            }
            if let Some(part) = read_part(res, p) {
                rig.parts.push(part);
            }
        }
    }
    let spring_count = u32at(at + 44)? as usize;
    let spring_kind = u32at(at + 48)?;
    if spring_count <= 1000 && (spring_kind == 1 || spring_kind == 3) {
        let table = at + 40 + u32at(at + 40)? as usize;
        for i in 0..spring_count {
            let slot = table + i * 4;
            let Some(rel) = u32at(slot) else { break };
            if let Some(sp) = read_spring(res, slot + rel as usize) {
                rig.springs.push(sp);
            }
        }
    }
    let con_count = u32at(at + 36)? as usize;
    if con_count <= 1000 {
        let mut o = at + 32 + u32at(at + 32)? as usize;
        for _ in 0..con_count {
            let Some((con, size)) = read_constraint(res, o) else {
                break;
            };
            rig.constraints.push(con);
            o += size;
        }
    }
    Some(rig)
}

/// Returns the byte size consumed, because records pack back to back with a per-`kind` tail length.
fn read_constraint(res: &[u8], at: usize) -> Option<(Constraint, usize)> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let kind = u32at(at)?;
    let mut joints: Vec<String> = (0..3)
        .filter_map(|i| rel_cstr(res, at + 4 + i * 4))
        .collect();
    let params: Vec<f32> = (0..6)
        .map(|i| f32at(at + 16 + i * 4))
        .collect::<Option<_>>()?;
    let mut p = at + 40;
    let take_f = |n: usize, p: &mut usize| -> Option<Vec<f32>> {
        let v = (0..n)
            .map(|i| f32at(*p + i * 4))
            .collect::<Option<Vec<f32>>>()?;
        *p += n * 4;
        Some(v)
    };
    let (extra_floats, extra_ints) = match kind {
        1 => (take_f(3, &mut p)?, read_ints(res, &mut p, 2)?),
        3 => (take_f(4, &mut p)?, read_ints(res, &mut p, 2)?),
        5 => (take_f(7, &mut p)?, Vec::new()),
        7 => (take_f(2, &mut p)?, read_ints(res, &mut p, 3)?),
        8 => (Vec::new(), read_ints(res, &mut p, 1)?),
        11 => {
            joints.push(rel_cstr(res, p).unwrap_or_default());
            p += 4;
            (take_f(6, &mut p)?, read_ints(res, &mut p, 2)?)
        }
        12 => {
            joints.push(rel_cstr(res, p).unwrap_or_default());
            p += 4;
            let head = take_f(3, &mut p)?;
            joints.push(rel_cstr(res, p).unwrap_or_default());
            p += 4;
            let mut tail = take_f(5, &mut p)?;
            let mut all = head;
            all.append(&mut tail);
            (all, read_ints(res, &mut p, 2)?)
        }
        _ => (Vec::new(), Vec::new()),
    };
    let con = Constraint {
        kind,
        joints,
        params: params.try_into().ok()?,
        extra_floats,
        extra_ints,
    };
    Some((con, p - at))
}

fn read_ints(res: &[u8], p: &mut usize, n: usize) -> Option<Vec<u32>> {
    let v = (0..n)
        .map(|i| res.get(*p + i * 4..*p + i * 4 + 4).map(LE::read_u32))
        .collect::<Option<Vec<u32>>>()?;
    *p += n * 4;
    Some(v)
}

fn read_part(res: &[u8], p: usize) -> Option<Part> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let mut nodes = Vec::new();
    let node_count = u32at(p + 12)? as usize;
    if node_count <= 4096 {
        let base = p + 8 + u32at(p + 8)? as usize;
        for i in 0..node_count {
            let q = base + i * 64;
            if q + 64 > res.len() {
                break;
            }
            nodes.push(read_part_node(res, q)?);
        }
    }
    let mut links = Vec::new();
    let link_count = u32at(p + 20)? as usize;
    if link_count <= 8192 {
        let base = p + 16 + u32at(p + 16)? as usize;
        for i in 0..link_count {
            let l = base + i * 36;
            if l + 36 > res.len() {
                break;
            }
            links.push(PartLink {
                a: u32at(l)? as usize,
                b: u32at(l + 4)? as usize,
                kind: u32at(l + 20)?,
                rest: f32at(l + 24)?,
            });
        }
    }
    let mut shear = Vec::new();
    let shear_count = u32at(p + 72)? as usize;
    if shear_count <= 8192 {
        let base = p + 68 + u32at(p + 68)? as usize;
        for i in (0..shear_count.saturating_sub(1)).step_by(2) {
            let o = base + i * 2;
            let (Some(a), Some(b)) = (
                res.get(o..o + 2).map(LE::read_i16),
                res.get(o + 2..o + 4).map(LE::read_i16),
            ) else {
                break;
            };
            shear.push((a, b));
        }
    }
    let params: Vec<f32> = (0..8)
        .map(|i| f32at(p + 36 + i * 4))
        .collect::<Option<_>>()?;
    Some(Part {
        name: rel_cstr(res, p).unwrap_or_default(),
        nodes,
        links,
        structural: u32at(p + 24)? as usize,
        shear,
        params: params.try_into().ok()?,
        colliders: read_constraint_table(res, p + 28, u32at(p + 32)? as usize),
    })
}

fn read_part_node(res: &[u8], q: usize) -> Option<PartNode> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let v3 = |o: usize| -> Option<[f32; 3]> { Some([f32at(o)?, f32at(o + 4)?, f32at(o + 8)?]) };
    let rest = match u32at(q + 52)? {
        0 => None,
        rel => {
            let r = q + 52 + rel as usize;
            (r + 32 <= res.len())
                .then(|| {
                    Some((
                        [f32at(r)?, f32at(r + 4)?, f32at(r + 8)?, f32at(r + 12)?],
                        v3(r + 16)?,
                    ))
                })
                .flatten()
        }
    };
    Some(PartNode {
        joint: rel_cstr(res, q).unwrap_or_default(),
        params: v3(q + 20)?,
        params2: v3(q + 36)?,
        rest,
        colliders: read_constraint_table(res, q + 4, u32at(q + 8)? as usize),
        forces: read_force_table(res, q + 12, u32at(q + 16)? as usize),
    })
}

fn read_constraint_table(res: &[u8], at: usize, count: usize) -> Vec<Constraint> {
    let mut out = Vec::new();
    if count > 1000 {
        return out;
    }
    let Some(rel) = res.get(at..at + 4).map(LE::read_u32) else {
        return out;
    };
    let table = at + rel as usize;
    for i in 0..count {
        let slot = table + i * 4;
        let Some(r) = res.get(slot..slot + 4).map(LE::read_u32) else {
            break;
        };
        if let Some((c, _)) = read_constraint(res, slot + r as usize) {
            out.push(c);
        }
    }
    out
}

fn read_force_table(res: &[u8], at: usize, count: usize) -> Vec<Force> {
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let mut out = Vec::new();
    if count > 1000 {
        return out;
    }
    let Some(rel) = res.get(at..at + 4).map(LE::read_u32) else {
        return out;
    };
    let table = at + rel as usize;
    for i in 0..count {
        let slot = table + i * 4;
        let Some(r) = res.get(slot..slot + 4).map(LE::read_u32) else {
            break;
        };
        let f = slot + r as usize;
        if f + 32 > res.len() {
            break;
        }
        let (Some(x), Some(y), Some(z), Some(m)) =
            (f32at(f + 16), f32at(f + 20), f32at(f + 24), f32at(f + 28))
        else {
            break;
        };
        out.push(Force {
            name: rel_cstr(res, f).unwrap_or_default(),
            reference: rel_cstr(res, f + 8).unwrap_or_default(),
            direction: [x, y, z],
            magnitude: m,
        });
    }
    out
}

fn read_spring(res: &[u8], at: usize) -> Option<Spring> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let i16at = |o: usize| res.get(o..o + 2).map(LE::read_i16);
    let attach = |g: usize, j: usize| -> Option<Attach> {
        let (g, j) = (i16at(g)?, i16at(j)?);
        (g >= 0 && j >= 0).then_some(Attach {
            group: g as usize,
            joint: j as usize,
        })
    };
    let name = rel_cstr(res, at).unwrap_or_default();
    let at0 = attach(at + 12, at + 14)?;
    match u32at(at + 8)? {
        1 => Some(Spring::Pin {
            name,
            at: at0,
            anchor: rel_cstr(res, at + 16).unwrap_or_default(),
        }),
        2 => Some(Spring::Link {
            name,
            a: at0,
            b: attach(at + 16, at + 18)?,
            rest: f32at(at + 20)?,
            stiffness: f32at(at + 24)?,
        }),
        3 => Some(Spring::Nail {
            name,
            at: at0,
            anchor: rel_cstr(res, at + 16).unwrap_or_default(),
            offset: [f32at(at + 20)?, f32at(at + 24)?, f32at(at + 28)?],
            strength: f32at(at + 36)?,
        }),
        _ => None,
    }
}

/// Byte offsets, from the record start, of each field in one [`IkKind`]'s body.
struct IkLayout {
    joints: &'static [usize],
    /// Two-bone records store all three maxima before all three minima, so pairs are not adjacent.
    limits: &'static [(usize, usize)],
    /// `(offset, word count)` runs.
    params: &'static [(usize, usize)],
    flags: &'static [usize],
    /// Offset of the descriptor addressing the nested records, when there is one.
    children: Option<usize>,
}

/// Shipped files nest one level; the cap only stops a malformed file looping.
const IK_MAX_DEPTH: usize = 4;

impl IkKind {
    fn from_raw(v: u16) -> IkKind {
        match v {
            0x201 => IkKind::Arm,
            0x202 => IkKind::LookAt,
            0x203 => IkKind::LookAtJoint,
            0x204 => IkKind::Aim,
            0x205 => IkKind::AimFollow,
            0x206 => IkKind::TwoBone,
            0x209 => IkKind::JointPlace,
            0x20a => IkKind::HipAdjust,
            0x20b => IkKind::HipRotate,
            other => IkKind::Unknown(other),
        }
    }

    fn layout(self) -> Option<&'static IkLayout> {
        const FOLLOWER: IkLayout = IkLayout {
            joints: &[12],
            limits: &[],
            params: &[(16, 24), (116, 1)],
            flags: &[112],
            children: None,
        };
        Some(match self {
            IkKind::Arm => &IkLayout {
                joints: &[12, 16, 20],
                limits: &[],
                params: &[(24, 8)],
                flags: &[],
                children: None,
            },
            IkKind::LookAt => &IkLayout {
                joints: &[20, 112],
                limits: &[(36, 40), (44, 48), (52, 56), (60, 64), (68, 72), (76, 80)],
                params: &[(24, 3), (84, 7), (116, 2), (128, 2), (140, 4), (160, 2)],
                flags: &[124, 136, 156],
                children: Some(12),
            },
            IkKind::LookAtJoint | IkKind::AimFollow => &FOLLOWER,
            IkKind::Aim => &IkLayout {
                joints: &[20, 24],
                limits: &[(40, 44), (48, 52)],
                params: &[(28, 3), (56, 3), (72, 4)],
                flags: &[68],
                children: Some(12),
            },
            IkKind::TwoBone => &IkLayout {
                joints: &[12, 16, 20, 24],
                limits: &[(100, 88), (104, 92), (108, 96)],
                params: &[(28, 7), (60, 2), (72, 1), (112, 7), (144, 5)],
                flags: &[140],
                children: None,
            },
            IkKind::JointPlace => &IkLayout {
                joints: &[12, 16],
                limits: &[],
                params: &[(20, 22), (112, 3)],
                flags: &[108],
                children: None,
            },
            IkKind::HipAdjust => &IkLayout {
                joints: &[12],
                limits: &[],
                params: &[(16, 2), (36, 1)],
                flags: &[32],
                children: Some(24),
            },
            IkKind::HipRotate => &IkLayout {
                joints: &[12],
                limits: &[],
                params: &[(16, 1), (28, 1)],
                flags: &[],
                children: Some(20),
            },
            IkKind::Unknown(_) => return None,
        })
    }
}

fn read_ik_rig(res: &[u8], at: usize) -> Option<IkRig> {
    let count = res.get(at + 28..at + 32).map(LE::read_u32)? as usize;
    let mut nodes = Vec::new();
    if count <= 1000 {
        for r in ik_records(res, at + 24, count)? {
            if let Some(n) = read_ik_node(res, r, 0) {
                nodes.push(n);
            }
        }
    }
    Some(IkRig {
        name: rel_cstr(res, at + 4).unwrap_or_default(),
        flag: res.get(at + 12..at + 14).map(LE::read_u16)?,
        nodes,
    })
}

/// The slots are offsets relative to themselves, as at every other level; `TRBLib` reads them as
/// plain values here, which is why it drops the nested records.
fn ik_records(res: &[u8], at: usize, count: usize) -> Option<Vec<usize>> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let table = at + u32at(at)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let slot = table + i * 4;
        let Some(rel) = u32at(slot) else { break };
        out.push(slot + rel as usize);
    }
    Some(out)
}

fn read_ik_node(res: &[u8], r: usize, depth: usize) -> Option<IkNode> {
    let u32at = |o: usize| res.get(o..o + 4).map(LE::read_u32);
    let f32at = |o: usize| res.get(o..o + 4).map(LE::read_f32);
    let kind = IkKind::from_raw(res.get(r + 8..r + 10).map(LE::read_u16)?);
    let mut node = IkNode {
        kind,
        name: rel_cstr(res, r).unwrap_or_default(),
        joints: Vec::new(),
        limits: Vec::new(),
        plant: None,
        params: Vec::new(),
        flags: Vec::new(),
        children: Vec::new(),
    };
    let Some(l) = kind.layout() else {
        return Some(node);
    };
    for &o in l.joints {
        let name = rel_cstr(res, r + o)?;
        if !name.is_empty() {
            node.joints.push(name);
        }
    }
    for &(min, max) in l.limits {
        node.limits.push(AngleLimit {
            min: f32at(r + min)?,
            max: f32at(r + max)?,
        });
    }
    for &(o, n) in l.params {
        for k in 0..n {
            node.params.push(f32at(r + o + k * 4)?);
        }
    }
    for &o in l.flags {
        node.flags.push(u32at(r + o)?);
    }
    if kind == IkKind::TwoBone {
        node.plant = Some(FootPlant {
            height: f32at(r + 56)?,
            raised_height: f32at(r + 68)?,
            up: [f32at(r + 76)?, f32at(r + 80)?, f32at(r + 84)?],
        });
    }
    if let Some(o) = l.children {
        let count = u32at(r + o + 4)? as usize;
        if depth < IK_MAX_DEPTH && count <= 1000 {
            for c in ik_records(res, r + o, count)? {
                if let Some(n) = read_ik_node(res, c, depth + 1) {
                    node.children.push(n);
                }
            }
        }
    }
    Some(node)
}

fn rel_cstr(res: &[u8], at: usize) -> Option<String> {
    let rel = res.get(at..at + 4).map(LE::read_u32)? as usize;
    cstr(res, at + rel)
}

fn cstr(res: &[u8], at: usize) -> Option<String> {
    let rest = res.get(at..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth() -> Vec<u8> {
        synth_group(2)
    }

    /// Laid out the way the shipped files do.
    fn synth_group(entry_type: u32) -> Vec<u8> {
        let mut d = vec![0u8; 72];
        d[..8].copy_from_slice(b"SEDBPHB\0");
        LE::write_u32(&mut d[64..68], 1);

        let table = 72;
        d.extend_from_slice(&[0; 4]);
        let entry = d.len();
        LE::write_u32(&mut d[table..table + 4], (entry - table) as u32);

        d.extend_from_slice(&[0; 40]);
        LE::write_u32(&mut d[entry..entry + 4], entry_type);
        LE::write_u32(&mut d[entry + 28..entry + 32], 1);

        let array = d.len();
        LE::write_u32(
            &mut d[entry + 24..entry + 28],
            (array - (entry + 24)) as u32,
        );
        d.extend_from_slice(&[0; 12]);

        let rec = d.len();
        LE::write_u32(&mut d[array..array + 4], (rec - array) as u32);
        d.extend_from_slice(&[0; 60]);
        LE::write_u32(&mut d[rec..rec + 4], 3);
        let body = rec + 12;
        for (i, v) in [0.5f32, 1.5, 2.5].iter().enumerate() {
            LE::write_f32(&mut d[body + 4 + i * 4..body + 8 + i * 4], *v);
        }
        for (i, v) in [0.1f32, 0.2, 0.3].iter().enumerate() {
            LE::write_f32(&mut d[body + 16 + i * 4..body + 20 + i * 4], *v);
        }
        LE::write_f32(&mut d[body + 28..body + 32], 0.25);
        LE::write_f32(&mut d[body + 32..body + 36], 7.0);

        let mut put = |s: &[u8]| {
            let at = d.len();
            d.extend_from_slice(s);
            d.push(0);
            at
        };
        let group_name = put(b"overlap_001");
        let shape_name = put(b"phy_head001");
        let joint_name = put(b"head");
        let vol_name = put(b"body_001");
        LE::write_u64(
            &mut d[entry + 4..entry + 12],
            (group_name - (entry + 4)) as u64,
        );
        LE::write_u32(&mut d[rec + 4..rec + 8], (shape_name - (rec + 4)) as u32);
        LE::write_u32(&mut d[rec + 8..rec + 12], (joint_name - (rec + 8)) as u32);
        LE::write_u64(
            &mut d[array + 4..array + 12],
            (vol_name - (array + 4)) as u64,
        );
        d
    }

    #[test]
    fn decodes_a_sphere_volume() {
        let phb = Phb::parse(&synth()).expect("parses");
        assert_eq!(phb.groups.len(), 1);
        let g = &phb.groups[0];
        assert_eq!(g.name, "overlap_001");
        assert_eq!(g.volumes.len(), 1);
        let v = &g.volumes[0];
        assert_eq!(v.shape, Shape::Capsule);
        assert_eq!(v.shape_name, "phy_head001");
        assert_eq!(v.joint, "head");
        assert_eq!(v.name, "body_001");
        assert_eq!(v.offset, [0.5, 1.5, 2.5]);
        assert_eq!(v.rotation, [0.1, 0.2, 0.3]);
        assert_eq!(v.radius, 0.25);
        assert_eq!(v.extra[0], 7.0);
    }

    #[test]
    fn a_type_1_entry_decodes_as_a_world_collision_capsule() {
        let phb = Phb::parse(&synth_group(1)).expect("parses");
        assert!(phb.groups.is_empty());
        assert_eq!(phb.world_collision.len(), 1);
        let g = &phb.world_collision[0];
        assert_eq!(g.name, "overlap_001");
        assert_eq!(g.volumes.len(), 1);
        assert_eq!(g.volumes[0].shape, Shape::Capsule);
        assert_eq!(g.volumes[0].joint, "head");
        assert_eq!(
            phb.volumes().count(),
            0,
            "world collision stays out of volumes()"
        );
    }

    fn synth_ik() -> Vec<u8> {
        let mut d = vec![0u8; 72];
        d[..8].copy_from_slice(b"SEDBPHB\0");
        LE::write_u32(&mut d[64..68], 1);

        let table = 72;
        d.extend_from_slice(&[0; 4]);
        let entry = d.len();
        LE::write_u32(&mut d[table..table + 4], (entry - table) as u32);

        d.extend_from_slice(&[0; 36]);
        LE::write_u32(&mut d[entry..entry + 4], 7);
        LE::write_u16(&mut d[entry + 12..entry + 14], 1);
        LE::write_u32(&mut d[entry + 28..entry + 32], 1);

        let list = d.len();
        LE::write_u32(&mut d[entry + 24..entry + 28], (list - (entry + 24)) as u32);
        d.extend_from_slice(&[0; 4]);

        let hip = d.len();
        LE::write_u32(&mut d[list..list + 4], (hip - list) as u32);
        d.extend_from_slice(&[0; 32]);
        LE::write_u16(&mut d[hip + 8..hip + 10], 0x20b);
        LE::write_u16(&mut d[hip + 10..hip + 12], 1);
        LE::write_f32(&mut d[hip + 16..hip + 20], 0.3);
        LE::write_u32(&mut d[hip + 24..hip + 28], 1);

        let sub = d.len();
        LE::write_u32(&mut d[hip + 20..hip + 24], (sub - (hip + 20)) as u32);
        d.extend_from_slice(&[0; 4]);

        let leg = d.len();
        LE::write_u32(&mut d[sub..sub + 4], (leg - sub) as u32);
        d.extend_from_slice(&[0; 164]);
        LE::write_u16(&mut d[leg + 8..leg + 10], 0x206);
        LE::write_u16(&mut d[leg + 10..leg + 12], 1);
        LE::write_f32(&mut d[leg + 56..leg + 60], 0.129);
        LE::write_f32(&mut d[leg + 68..leg + 72], 0.149);
        for (i, v) in [-0.6f32, -0.8, 0.0].iter().enumerate() {
            LE::write_f32(&mut d[leg + 76 + i * 4..leg + 80 + i * 4], *v);
        }
        for (i, v) in [10.0f32, 20.0, 110.0, -10.0, -20.0, -120.0]
            .iter()
            .enumerate()
        {
            LE::write_f32(&mut d[leg + 88 + i * 4..leg + 92 + i * 4], *v);
        }
        LE::write_u32(&mut d[leg + 140..leg + 144], 1);

        let mut put = |s: &[u8]| {
            let at = d.len();
            d.extend_from_slice(s);
            d.push(0);
            at
        };
        let names = [
            put(b"i_2j"),
            put(b"hip_rot_ik__001"),
            put(b"hip"),
            put(b"L"),
            put(b"L_femur"),
            put(b"L_tibia"),
            put(b"L_foot"),
            put(b""),
        ];
        for (at, name) in [
            (entry + 4, names[0]),
            (hip, names[1]),
            (hip + 12, names[2]),
            (leg, names[3]),
            (leg + 12, names[4]),
            (leg + 16, names[5]),
            (leg + 20, names[6]),
            (leg + 24, names[7]),
        ] {
            LE::write_u32(&mut d[at..at + 4], (name - at) as u32);
        }
        d
    }

    #[test]
    fn decodes_a_two_bone_leg_under_a_hip_solver() {
        let phb = Phb::parse(&synth_ik()).expect("parses");
        assert_eq!(phb.ik_rigs.len(), 1);
        let rig = &phb.ik_rigs[0];
        assert_eq!(rig.name, "i_2j");
        assert_eq!(rig.flag, 1);
        assert_eq!(rig.nodes.len(), 1);

        let hip = &rig.nodes[0];
        assert_eq!(hip.kind, IkKind::HipRotate);
        assert_eq!(hip.name, "hip_rot_ik__001");
        assert_eq!(hip.joints, ["hip"]);
        // TRBLib reads this 0.3 as a u32, which is the tell that its type-523 layout is off.
        assert_eq!(hip.params[0], 0.3);
        assert_eq!(hip.children.len(), 1);

        let leg = &hip.children[0];
        assert_eq!(leg.kind, IkKind::TwoBone);
        assert_eq!(leg.name, "L");
        assert_eq!(
            leg.joints,
            ["L_femur", "L_tibia", "L_foot"],
            "the blank 4th slot is dropped"
        );
        let plant = leg.plant.expect("two-bone nodes carry a plant");
        assert_eq!(plant.height, 0.129);
        assert_eq!(plant.raised_height, 0.149);
        assert_eq!(plant.up, [-0.6, -0.8, 0.0]);
        let limits: Vec<(f32, f32)> = leg.limits.iter().map(|l| (l.min, l.max)).collect();
        assert_eq!(limits, [(-10.0, 10.0), (-20.0, 20.0), (-120.0, 110.0)]);
        assert_eq!(leg.flags, [1]);
        assert_eq!(phb.ik_nodes().count(), 2);
    }

    #[test]
    fn rejects_foreign_resources() {
        assert!(Phb::parse(b"SEDBshd\0................").is_none());
        assert!(Phb::parse(b"").is_none());
    }

    #[test]
    fn truncation_does_not_panic() {
        for full in [synth(), synth_group(1), synth_ik()] {
            for cut in 0..full.len() {
                let _ = Phb::parse(&full[..cut]);
            }
        }
    }

    #[test]
    fn shipped_volumes_are_coherent() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let base = format!("{dir}/chr/pc/c201/bin/c201.win32.trb");
        let Ok(bytes) = std::fs::read(&base) else {
            return;
        };
        let Ok(t) = crate::trb::Trb::parse(&bytes) else {
            return;
        };
        let joints: Vec<String> = (0..t.resource_names().len())
            .filter_map(|r| t.resource_data(r))
            .filter(|d| d.starts_with(b"SEDBSKL"))
            .filter_map(crate::skl::Skeleton::parse)
            .flat_map(|s| s.joints.iter().map(|j| j.name.clone()).collect::<Vec<_>>())
            .collect();
        assert!(!joints.is_empty(), "c201 should have a skeleton");
        let mut seen = 0;
        for r in 0..t.resource_names().len() {
            let Some(d) = t.resource_data(r) else {
                continue;
            };
            let Some(phb) = Phb::parse(d) else { continue };
            for v in phb.volumes() {
                seen += 1;
                assert!(
                    joints.contains(&v.joint),
                    "volume {} names unknown joint {}",
                    v.name,
                    v.joint
                );
                assert!(
                    v.radius.is_finite() && v.radius > 0.0 && v.radius < 100.0,
                    "bad radius {}",
                    v.radius
                );
                assert!(v.offset.iter().all(|c| c.is_finite()));
            }
        }
        assert!(seen > 0, "c201 should ship collision volumes");
    }

    #[test]
    fn shipped_rigs_bind_to_the_skeleton_by_name() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/chr/pc/c003/bin/c003.win32.trb");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let Ok(t) = crate::trb::Trb::parse(&bytes) else {
            return;
        };
        let joints: Vec<String> = (0..t.resource_names().len())
            .filter_map(|r| t.resource_data(r))
            .filter(|d| d.starts_with(b"SEDBSKL"))
            .filter_map(crate::skl::Skeleton::parse)
            .flat_map(|s| s.joints.iter().map(|j| j.name.clone()).collect::<Vec<_>>())
            .collect();
        assert!(!joints.is_empty());

        let rigs: Vec<Rig> = (0..t.resource_names().len())
            .filter_map(|r| t.resource_data(r))
            .filter_map(Phb::parse)
            .flat_map(|p| p.rigs)
            .collect();
        assert!(!rigs.is_empty(), "c003 should ship type-4 rigs");
        assert!(
            rigs.iter().any(|r| r.name.contains("hair")),
            "expected a hair rig, got {:?}",
            rigs.iter().map(|r| &r.name).collect::<Vec<_>>()
        );

        let core = |n: &str| -> String {
            let s = n
                .strip_prefix("chain_")
                .or_else(|| n.strip_prefix("pin_"))
                .unwrap_or(n);
            s.rsplit_once('_').map(|(a, _)| a).unwrap_or(s).to_string()
        };
        let (mut hit, mut total) = (0, 0);
        for r in &rigs {
            for c in &r.chains {
                assert!(!c.name.is_empty(), "chain names must decode");
                assert!(
                    c.params.iter().all(|p| p.is_finite()),
                    "chain params must be finite"
                );
                total += 1;
                if joints.contains(&core(&c.name)) {
                    hit += 1;
                }
            }
            for sp in &r.springs {
                assert!(!sp.name().is_empty(), "spring names must decode");
                total += 1;
                if joints.contains(&core(sp.name())) {
                    hit += 1;
                }
            }
        }
        assert!(
            total >= 50,
            "expected a substantial rig, got {total} entries"
        );
        let pct = 100 * hit / total;
        assert!(
            pct >= 85,
            "only {pct}% of rig names resolved to joints ({hit}/{total})"
        );
    }

    #[test]
    fn shipped_cloth_parts_and_springs_are_coherent() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let mut totals = [0usize; 5];
        for id in ["c003", "c201"] {
            let p = format!("{dir}/chr/pc/{id}/bin/{id}.win32.trb");
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let Ok(t) = crate::trb::Trb::parse(&bytes) else {
                continue;
            };
            let joints: Vec<String> = (0..t.resource_names().len())
                .filter_map(|r| t.resource_data(r))
                .filter(|d| d.starts_with(b"SEDBSKL"))
                .filter_map(crate::skl::Skeleton::parse)
                .flat_map(|s| s.joints.iter().map(|j| j.name.clone()).collect::<Vec<_>>())
                .collect();
            assert!(!joints.is_empty());

            for phb in (0..t.resource_names().len())
                .filter_map(|r| t.resource_data(r))
                .filter_map(Phb::parse)
            {
                for rig in &phb.rigs {
                    let groups: Vec<usize> = rig
                        .chains
                        .iter()
                        .map(|c| c.joints.len())
                        .chain(rig.parts.iter().map(|p| p.nodes.len()))
                        .collect();
                    for part in &rig.parts {
                        totals[0] += 1;
                        for n in &part.nodes {
                            totals[1] += 1;
                            assert!(
                                joints.contains(&n.joint),
                                "part node names unknown joint {}",
                                n.joint
                            );
                            if let Some((q, d)) = n.rest {
                                let ql = q.iter().map(|x| x * x).sum::<f32>().sqrt();
                                let dl = d.iter().map(|x| x * x).sum::<f32>().sqrt();
                                assert!((ql - 1.0).abs() < 0.01, "rest quaternion not unit: {ql}");
                                assert!((dl - 1.0).abs() < 0.01, "rest direction not unit: {dl}");
                            }
                            for f in &n.forces {
                                totals[2] += 1;
                                let l = f.direction.iter().map(|x| x * x).sum::<f32>().sqrt();
                                assert!((l - 1.0).abs() < 0.01, "force direction not unit: {l}");
                            }
                        }
                        assert!(part.structural <= part.links.len());
                        for l in &part.links {
                            totals[3] += 1;
                            assert!(
                                l.a < part.nodes.len() && l.b < part.nodes.len(),
                                "link out of range"
                            );
                            assert!(
                                l.rest.is_finite() && l.rest >= 0.0,
                                "bad rest length {}",
                                l.rest
                            );
                        }
                        for l in part.links.iter().take(part.structural) {
                            assert_eq!(
                                (l.a as i64 - l.b as i64).abs(),
                                1,
                                "structural link is not adjacent"
                            );
                        }
                        for (a, b) in &part.shear {
                            let n = part.nodes.len() as i16;
                            assert!(
                                *a >= 0 && *a < n && *b >= 0 && *b < n,
                                "shear index out of range"
                            );
                        }
                    }
                    for sp in &rig.springs {
                        totals[4] += 1;
                        let ok = |a: &Attach| groups.get(a.group).is_some_and(|n| a.joint < *n);
                        match sp {
                            Spring::Pin { at, .. } | Spring::Nail { at, .. } => {
                                assert!(ok(at), "spring {} addresses a missing joint", sp.name())
                            }
                            Spring::Link {
                                a,
                                b,
                                rest,
                                stiffness,
                                ..
                            } => {
                                assert!(
                                    ok(a) && ok(b),
                                    "spring {} addresses a missing joint",
                                    sp.name()
                                );
                                assert_ne!(a, b, "spring {} links a joint to itself", sp.name());
                                assert!(*rest > 0.0, "spring {} has rest {rest}", sp.name());
                                assert!(
                                    (0.0..=1.0).contains(stiffness),
                                    "stiffness {stiffness} out of range"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(totals[0] > 10, "expected cloth parts, got {}", totals[0]);
        assert!(totals[1] > 100, "expected part nodes, got {}", totals[1]);
        assert!(totals[3] > 100, "expected links, got {}", totals[3]);
        assert!(totals[4] > 100, "expected springs, got {}", totals[4]);
    }

    fn corpus_trbs() -> Vec<std::path::PathBuf> {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return Vec::new();
        };
        let (mut out, mut stack) = (Vec::new(), vec![std::path::PathBuf::from(dir)]);
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

    fn bundle_joints(t: &crate::trb::Trb) -> Vec<String> {
        (0..t.resource_count())
            .filter_map(|r| t.resource_data(r))
            .filter(|d| d.starts_with(b"SEDBSKL"))
            .filter_map(crate::skl::Skeleton::parse)
            .flat_map(|s| s.joints.into_iter().map(|j| j.name))
            .collect()
    }

    #[test]
    fn shipped_ik_rigs_are_coherent() {
        let files = corpus_trbs();
        if files.is_empty() {
            return;
        }
        let mut census = std::collections::BTreeMap::<String, u64>::new();
        let (mut rigs, mut nodes, mut resolved, mut unskinned) = (0u64, 0u64, 0u64, 0u64);
        let mut missing: Vec<String> = Vec::new();
        for p in &files {
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            let Ok(t) = crate::trb::Trb::parse(&bytes) else {
                continue;
            };
            let joints = bundle_joints(&t);
            for phb in (0..t.resource_count())
                .filter_map(|r| t.resource_data(r))
                .filter_map(Phb::parse)
            {
                rigs += phb.ik_rigs.len() as u64;
                for rig in &phb.ik_rigs {
                    assert!(!rig.name.is_empty(), "unnamed IK rig in {}", p.display());
                    assert!(rig.flag <= 1, "IK rig flag {} in {}", rig.flag, p.display());
                }
                for n in phb.ik_nodes() {
                    nodes += 1;
                    *census.entry(format!("{:?}", n.kind)).or_default() += 1;
                    assert!(
                        !matches!(n.kind, IkKind::Unknown(_)),
                        "unknown IK kind {:?} in {}",
                        n.kind,
                        p.display()
                    );
                    assert!(
                        n.params.iter().all(|v| v.is_finite()),
                        "non-finite param in {}",
                        p.display()
                    );
                    let (want_joints, want_limits) = match n.kind {
                        IkKind::Arm => (2, 0),
                        IkKind::LookAt => (1, 6),
                        IkKind::Aim => (2, 2),
                        IkKind::TwoBone => (3, 3),
                        IkKind::JointPlace | IkKind::LookAtJoint | IkKind::AimFollow => (1, 0),
                        IkKind::HipAdjust | IkKind::HipRotate => (1, 0),
                        IkKind::Unknown(_) => (n.joints.len(), n.limits.len()),
                    };
                    assert_eq!(
                        n.joints.len(),
                        want_joints,
                        "{:?} joints in {}",
                        n.kind,
                        p.display()
                    );
                    assert_eq!(
                        n.limits.len(),
                        want_limits,
                        "{:?} limits in {}",
                        n.kind,
                        p.display()
                    );
                    assert_eq!(
                        n.plant.is_some(),
                        n.kind == IkKind::TwoBone,
                        "{:?} plant in {}",
                        n.kind,
                        p.display()
                    );
                    let nested = matches!(
                        n.kind,
                        IkKind::LookAt | IkKind::Aim | IkKind::HipAdjust | IkKind::HipRotate
                    );
                    assert!(
                        nested || n.children.is_empty(),
                        "{:?} has children in {}",
                        n.kind,
                        p.display()
                    );
                    for l in &n.limits {
                        assert!(l.min <= l.max, "limit {l:?} inverted in {}", p.display());
                        assert!(
                            (-360.0..=360.0).contains(&l.min) && (-360.0..=360.0).contains(&l.max),
                            "limit {l:?} out of degree range in {}",
                            p.display()
                        );
                    }
                    if let Some(plant) = n.plant {
                        assert_eq!(n.kind, IkKind::TwoBone);
                        let len = plant.up.iter().map(|c| c * c).sum::<f32>().sqrt();
                        assert!(
                            (len - 1.0).abs() < 0.01,
                            "ground normal not unit ({len}) in {}",
                            p.display()
                        );
                        assert!(
                            (0.0..10.0).contains(&plant.height)
                                && plant.raised_height >= plant.height,
                            "implausible plant {plant:?} in {}",
                            p.display()
                        );
                    }
                    if joints.is_empty() {
                        unskinned += 1;
                        continue;
                    }
                    for j in &n.joints {
                        if joints.contains(j) {
                            resolved += 1;
                        } else {
                            missing.push(format!("{j} ({})", p.display()));
                        }
                    }
                }
            }
        }
        eprintln!(
            "{rigs} IK rigs, {nodes} nodes ({unskinned} in skeleton-less bundles), {resolved} joint names resolved"
        );
        eprintln!("kinds: {census:?}");
        assert!(rigs > 0, "no type-7 entries found under FF13_MODELS_DIR");
        // c604 ships eyelid-follow records for centre-lid joints its own skeleton does not define.
        assert!(
            missing.len() as u64 * 1000 <= resolved,
            "{} of {} IK joint names miss the skeleton: {:?}",
            missing.len(),
            missing.len() as u64 + resolved,
            &missing[..missing.len().min(8)]
        );
        for kind in [
            "LookAt",
            "LookAtJoint",
            "Aim",
            "AimFollow",
            "TwoBone",
            "JointPlace",
            "HipAdjust",
            "HipRotate",
        ] {
            assert!(census.contains_key(kind), "no {kind} nodes decoded");
        }
    }

    #[test]
    fn shipped_world_collision_is_capsules_on_the_root() {
        let files = corpus_trbs();
        if files.is_empty() {
            return;
        }
        let (mut groups, mut volumes) = (0u64, 0u64);
        for p in &files {
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            let Ok(t) = crate::trb::Trb::parse(&bytes) else {
                continue;
            };
            let joints = bundle_joints(&t);
            for phb in (0..t.resource_count())
                .filter_map(|r| t.resource_data(r))
                .filter_map(Phb::parse)
            {
                for g in &phb.world_collision {
                    groups += 1;
                    assert_eq!(g.name, "p_collision_001", "in {}", p.display());
                    assert!(
                        !g.volumes.is_empty(),
                        "empty world collision in {}",
                        p.display()
                    );
                    for v in &g.volumes {
                        volumes += 1;
                        assert_eq!(v.shape, Shape::Capsule, "in {}", p.display());
                        assert_eq!(v.joint, "trans", "in {}", p.display());
                        assert!(
                            joints.is_empty() || joints.contains(&v.joint),
                            "in {}",
                            p.display()
                        );
                        assert!(
                            v.radius > 0.0 && v.radius < 10.0,
                            "radius {} in {}",
                            v.radius,
                            p.display()
                        );
                        assert!(
                            v.extra[0] > 0.0,
                            "capsule half-length {} in {}",
                            v.extra[0],
                            p.display()
                        );
                        assert!(
                            v.offset.iter().chain(&v.rotation).all(|c| c.is_finite()),
                            "non-finite transform in {}",
                            p.display()
                        );
                    }
                }
            }
        }
        eprintln!("{groups} world-collision groups, {volumes} capsules");
        assert!(groups > 0, "no type-1 entries found under FF13_MODELS_DIR");
    }
}
