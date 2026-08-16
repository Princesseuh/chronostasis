use std::path::{Path, PathBuf};

use ff13::formats::skl;
use glam::{Mat4, Quat, Vec3};

use ff13::formats::model::MAX_PALETTE;

pub(crate) struct Rig {
    pub(crate) joints: Vec<skl::Joint>,
    pub(crate) inv_bind: Vec<Mat4>,
    pub(crate) pose: Vec<Vec3>,
    pub(crate) selected: Option<usize>,
    pub(crate) anim: Option<ff13::formats::mot::Motion>,
    pub(crate) frame: f32,
    pub(crate) playing: bool,
    pub(crate) root_motion: bool,
}

impl Rig {
    pub(crate) fn new(skel: &skl::Skeleton) -> Rig {
        let inv_bind = skel
            .inverse_bind()
            .iter()
            .map(Mat4::from_cols_array_2d)
            .collect();
        Rig {
            joints: skel.joints.clone(),
            inv_bind,
            pose: vec![Vec3::ZERO; skel.joints.len()],
            selected: None,
            anim: None,
            frame: 0.0,
            playing: false,
            root_motion: false,
        }
    }

    pub(crate) fn posed_local(&self, i: usize) -> Mat4 {
        let bind = self.joints[i].local_matrix();
        let base = match &self.anim {
            Some(m) => Mat4::from_cols_array_2d(&m.local(i, self.frame, bind, self.root_motion)),
            None => Mat4::from_cols_array_2d(&bind),
        };
        let e = self.pose[i];
        base * Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, e.x, e.y, e.z))
    }

    pub(crate) fn posed_world(&self) -> Vec<Mat4> {
        let n = self.joints.len();
        let locals: Vec<Mat4> = (0..n).map(|i| self.posed_local(i)).collect();
        let mut world = locals.clone();
        for i in 0..n {
            let p = self.joints[i].parent;
            // Skeletons store parents before children, so one pass composes each chain.
            if p >= 0 && (p as usize) < i {
                world[i] = world[p as usize] * locals[i];
            }
        }
        world
    }

    pub(crate) fn solve(&self) -> (Vec<Mat4>, Vec<LineVertex>) {
        let world = self.posed_world();
        let skin = world
            .iter()
            .zip(&self.inv_bind)
            .map(|(w, ib)| *w * *ib)
            .collect();
        let pos = |i: usize| world[i].col(3).truncate();

        let (mut mn, mut mx) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for i in 0..world.len() {
            mn = mn.min(pos(i));
            mx = mx.max(pos(i));
        }
        let scale = (mx - mn).length().max(1e-3);

        let mut lines = Vec::new();
        let mut seg = |a: Vec3, b: Vec3, hot: f32| {
            lines.push(LineVertex {
                pos: a.to_array(),
                hot,
            });
            lines.push(LineVertex {
                pos: b.to_array(),
                hot,
            });
        };
        for (i, j) in self.joints.iter().enumerate() {
            if j.parent >= 0 && (j.parent as usize) < world.len() {
                let hot = (self.selected == Some(i) || self.selected == Some(j.parent as usize))
                    as u32 as f32;
                seg(pos(i), pos(j.parent as usize), hot);
            }
        }
        for i in 0..self.joints.len() {
            let sel = self.selected == Some(i);
            let r = scale * if sel { 0.022 } else { 0.004 };
            let hot = sel as u32 as f32;
            let p = pos(i);
            seg(p - Vec3::X * r, p + Vec3::X * r, hot);
            seg(p - Vec3::Y * r, p + Vec3::Y * r, hot);
            seg(p - Vec3::Z * r, p + Vec3::Z * r, hot);
        }
        (skin, lines)
    }
}

pub(crate) fn palette_bytes(palette: &[u32], skin: Option<&[Mat4]>) -> Vec<[[f32; 4]; 4]> {
    let mut out = Vec::new();
    palette_bytes_into(palette, skin, &mut out);
    out
}

pub(crate) fn palette_bytes_into(
    palette: &[u32],
    skin: Option<&[Mat4]>,
    out: &mut Vec<[[f32; 4]; 4]>,
) {
    let ident = Mat4::IDENTITY.to_cols_array_2d();
    out.clear();
    out.resize(MAX_PALETTE, ident);
    if let Some(skin) = skin {
        for (slot, &joint) in palette.iter().take(MAX_PALETTE).enumerate() {
            out[slot] = skin
                .get(joint as usize)
                .copied()
                .unwrap_or(Mat4::IDENTITY)
                .to_cols_array_2d();
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct LineVertex {
    pub(crate) pos: [f32; 3],
    pub(crate) hot: f32,
}

pub(crate) struct ClipRef {
    pub(crate) label: String,
    pub(crate) pack: PathBuf,
    pub(crate) name: String,
    pub(crate) supported: bool,
}

pub(crate) fn list_clips(trb_path: &Path) -> Vec<ClipRef> {
    let mut out = Vec::new();
    let Some(char_id) = trb_path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split('.').next())
    else {
        return out;
    };
    let mut root = trb_path.parent();
    while let Some(d) = root {
        if d.join("mot").is_dir() {
            break;
        }
        root = d.parent();
    }
    let Some(root) = root else { return out };
    let prefix = format!("sk_{char_id}_");
    for group in std::fs::read_dir(root.join("mot"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for sk in std::fs::read_dir(group.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if !sk.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let mut packs: Vec<PathBuf> = std::fs::read_dir(sk.path())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".white.win32.bin"))
                .collect();
            packs.sort();
            for p in packs {
                let pack = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.split('.').next())
                    .unwrap_or("");
                if let Ok(b) = std::fs::read(&p) {
                    for (name, supported) in ff13::formats::mot::Motion::clip_support(&b) {
                        out.push(ClipRef {
                            label: format!("{pack} · {name}"),
                            pack: p.clone(),
                            name,
                            supported,
                        });
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn load_clip(rig: &mut Rig, clip: &ClipRef) {
    rig.anim = std::fs::read(&clip.pack)
        .ok()
        .and_then(|b| ff13::formats::mot::Motion::from_wpd_named(&b, &clip.name));
    rig.frame = 0.0;
    rig.playing = false;
}
