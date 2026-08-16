//! Built-in model-swap recipes, combining game-shipped models at native resolution.

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::community::modelops::{
    load_models, merge_skeleton, pad16, rename_phb_bones, skeleton_and_phb,
};
use crate::formats::trb::Trb;

/// One output model slot: what to combine, which resources to pick, and how to merge skeletons.
pub struct TargetSwap {
    /// Data-dir-relative and extensionless, in `map` index order: donor first, target last.
    pub models: &'static [&'static str],
    /// Index into `models` of the slot whose structure the output extends.
    pub base_idx: usize,
    pub out_rel: &'static str,
    /// Output resource `i` comes from `models[map[i].0]` resource `map[i].1`.
    pub map: &'static [(u16, u16)],
    /// Byte-level, applied to resource data, e.g. `c_n910` to `c_c201`.
    pub mesh_renames: &'static [(&'static str, &'static str)],
    pub bone_renames: &'static [(&'static str, &'static str)],
    pub e_ops: &'static [(u16, u8, i32)],
    /// Fold the target's rig into the combined, donor-rigged skeleton.
    pub skel_merge: bool,
    pub skel_s: bool,
    /// Puts the target's face on the donor's body.
    pub face_fix: bool,
    /// Whole-name matching rather than substring.
    pub rename_exact: bool,
}

pub struct Recipe {
    pub character: &'static str,
    pub costume: &'static str,
    pub targets: &'static [TargetSwap],
}

pub fn find(character: &str, costume: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| {
        r.character.eq_ignore_ascii_case(character) && r.costume.eq_ignore_ascii_case(costume)
    })
}

/// The cutscene (`c0xx`) and field (`c2xx`) codes, for the generic swap.
pub fn character_codes(name: &str) -> Option<&'static [&'static str]> {
    Some(match name.to_ascii_lowercase().as_str() {
        "lightning" => &["c001", "c201"],
        "snow" => &["c002", "c202"],
        "sazh" => &["c003", "c203"],
        "hope" => &["c004", "c204"],
        "vanille" => &["c005", "c205"],
        "fang" => &["c006", "c206"],
        _ => return None,
    })
}

pub fn costumes_for(character: &str) -> Vec<&'static str> {
    RECIPES
        .iter()
        .filter(|r| r.character.eq_ignore_ascii_case(character))
        .map(|r| r.costume)
        .collect()
}

pub static RECIPES: &[Recipe] = &[
    // "hq" drops the higher-poly cutscene model into the field slot with its rig retargeted.
    Recipe {
        character: "lightning",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c001/bin/c001.win32", "chr/pc/c201/bin/c201.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c201/bin/c201.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (0, 28),
                (0, 29),
                (0, 30),
                (0, 31),
                (0, 32),
                (0, 33),
                (0, 34),
                (0, 35),
                (0, 36),
                (0, 37),
                (0, 38),
                (0, 39),
                (0, 40),
                (1, 33),
                (1, 34),
                (1, 35),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
                (0, 44),
                (1, 42),
                (1, 43),
                (1, 44),
                (1, 45),
                (1, 46),
            ],
            mesh_renames: &[("c_c001", "c_c201"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[
                ("L_hair1a", "L_dmhr1a"),
                ("L_hair4a", "L_dmhr4a"),
                ("R_hair1b", "R_dmhr1b"),
                ("R_hair3a", "R_dmhr3a"),
                ("R_hair5a", "R_dmhr5a"),
                ("L_hair2a", "L_dmhr2a"),
                ("R_hair1a", "R_dmhr1a"),
                ("R_hair2a", "R_dmhr2a"),
                ("R_hair4a", "R_dmhr4a"),
                ("T_hair1a", "T_dmhr1a"),
                ("T_hair2a", "T_dmhr2a"),
            ],
            e_ops: &[
                (4, 4, 9),
                (79, 4, -1),
                (10, 4, 11),
                (107, 4, -1),
                (11, 4, 12),
                (111, 4, -1),
                (13, 3, 16),
                (210, 4, -1),
                (15, 3, 25),
                (211, 4, -1),
                (36, 3, 37),
                (213, 4, -1),
                (50, 4, 51),
                (83, 4, -1),
                (51, 3, 52),
                (102, 4, -1),
                (53, 3, 88),
                (91, 4, -1),
                (59, 3, 93),
                (96, 4, -1),
                (79, 4, 108),
                (127, 4, -1),
                (114, 4, 122),
                (147, 4, -1),
                (33, 3, 134),
                (277, 3, -1),
                (42, 3, 43),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "sazh",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c003/bin/c003.win32", "chr/pc/c203/bin/c203.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c203/bin/c203.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (1, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (1, 26),
                (1, 27),
                (1, 28),
                (1, 29),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (1, 35),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
            ],
            mesh_renames: &[("c_c003", "c_c203"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[],
            e_ops: &[
                (39, 3, 42),
                (176, 4, -1),
                (42, 3, 53),
                (177, 4, -1),
                (41, 3, 73),
                (179, 4, -1),
                (4, 3, 108),
                (194, 4, -1),
                (29, 3, 155),
                (178, 4, -1),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "hope",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c004/bin/c004.win32", "chr/pc/c204/bin/c204.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c204/bin/c204.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (1, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (0, 28),
                (0, 29),
                (0, 30),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (1, 35),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_c004", "c_c204"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[],
            e_ops: &[
                (21, 3, 24),
                (123, 4, -1),
                (22, 3, 27),
                (129, 4, -1),
                (33, 3, 36),
                (192, 4, -1),
                (36, 3, 47),
                (193, 4, -1),
                (35, 3, 67),
                (195, 4, -1),
                (20, 4, 149),
                (194, 4, -1),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "vanille",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c005/bin/c005.win32", "chr/pc/c205/bin/c205.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c205/bin/c205.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (1, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (0, 28),
                (0, 29),
                (0, 30),
                (0, 31),
                (0, 32),
                (0, 33),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (1, 35),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_c005", "c_c205"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[],
            e_ops: &[
                (4, 3, 10),
                (17, 4, -1),
                (9, 3, 24),
                (147, 4, -1),
                (24, 3, 31),
                (202, 4, -1),
                (36, 3, 39),
                (200, 4, -1),
                (39, 3, 50),
                (201, 4, -1),
                (38, 3, 70),
                (203, 4, -1),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "snow",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c002/bin/c002.win32", "chr/pc/c202/bin/c202.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c202/bin/c202.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (1, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (0, 28),
                (0, 29),
                (0, 30),
                (0, 31),
                (0, 32),
                (0, 33),
                (0, 34),
                (0, 35),
                (0, 36),
                (0, 37),
                (0, 38),
                (0, 39),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
                (1, 44),
                (1, 45),
                (1, 46),
                (1, 47),
                (1, 48),
                (1, 49),
                (1, 50),
            ],
            mesh_renames: &[("c_c002", "c_c202"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[],
            e_ops: &[
                (31, 3, 34),
                (208, 4, -1),
                (34, 3, 45),
                (209, 4, -1),
                (33, 3, 65),
                (211, 4, -1),
                (20, 4, 169),
                (210, 4, -1),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "fang",
        costume: "hq",
        targets: &[TargetSwap {
            models: &["chr/pc/c006/bin/c006.win32", "chr/pc/c206/bin/c206.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c206/bin/c206.win32.trb",
            map: &[
                (0, 0),
                (0, 1),
                (1, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
                (0, 28),
                (0, 29),
                (0, 30),
                (1, 29),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (1, 35),
                (1, 36),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_c006", "c_c206"), ("c20x_puls", "c00x_puls")],
            bone_renames: &[],
            e_ops: &[
                (4, 3, 8),
                (105, 4, -1),
                (4, 4, 9),
                (107, 4, -1),
                (34, 3, 37),
                (198, 4, -1),
                (37, 3, 48),
                (199, 4, -1),
                (36, 3, 68),
                (201, 4, -1),
                (8, 3, 103),
                (197, 3, -1),
                (22, 4, 156),
                (200, 4, -1),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "lightning",
        costume: "lebreau",
        targets: &[
            TargetSwap {
                models: &["chr/npc/n910/bin/n910.win32", "chr/pc/c201/bin/c201.win32"],
                base_idx: 1,
                out_rel: "chr/pc/c201/bin/c201.win32.trb",
                map: &[
                    (0, 0),
                    (0, 1),
                    (0, 2),
                    (0, 3),
                    (0, 4),
                    (0, 5),
                    (0, 6),
                    (0, 7),
                    (0, 8),
                    (0, 9),
                    (0, 10),
                    (0, 11),
                    (0, 12),
                    (0, 13),
                    (0, 14),
                    (0, 15),
                    (0, 16),
                    (0, 17),
                    (0, 18),
                    (0, 19),
                    (1, 33),
                    (1, 34),
                    (1, 35),
                    (1, 36),
                    (1, 37),
                    (0, 24),
                    (0, 25),
                    (0, 26),
                    (0, 27),
                    (1, 44),
                    (1, 45),
                    (1, 46),
                ],
                mesh_renames: &[("c_n910", "c_c201")],
                bone_renames: &[("R_hair1a", "R_dmhr1a"), ("R_hair1b", "R_dmhr1b")],
                e_ops: &[],
                skel_merge: true,
                skel_s: true,
                face_fix: false,
                rename_exact: false,
            },
            TargetSwap {
                models: &["chr/npc/n910/bin/n910.win32", "chr/pc/c001/bin/c001.win32"],
                base_idx: 1,
                out_rel: "chr/pc/c001/bin/c001.win32.trb",
                map: &[
                    (0, 0),
                    (0, 1),
                    (0, 2),
                    (0, 3),
                    (0, 4),
                    (0, 5),
                    (0, 6),
                    (0, 7),
                    (0, 8),
                    (0, 9),
                    (0, 10),
                    (0, 11),
                    (0, 12),
                    (0, 13),
                    (0, 14),
                    (0, 15),
                    (0, 16),
                    (0, 17),
                    (0, 18),
                    (0, 19),
                    (1, 41),
                    (1, 42),
                    (1, 43),
                    (0, 24),
                    (0, 25),
                    (0, 26),
                    (0, 27),
                    (1, 45),
                    (1, 46),
                ],
                mesh_renames: &[("c_n910", "c_c001")],
                bone_renames: &[
                    ("L_hair1a", "L_dmhr1a"),
                    ("R_hair1a", "R_dmhr1a"),
                    ("R_hair1b", "R_dmhr1b"),
                ],
                e_ops: &[(64, 13, 0), (64, 14, 0), (64, 15, 0)],
                skel_merge: true,
                skel_s: true,
                face_fix: true,
                rename_exact: false,
            },
        ],
    },
    Recipe {
        character: "sazh",
        costume: "gadot",
        targets: &[TargetSwap {
            models: &["chr/npc/n909/bin/n909.win32", "chr/pc/c203/bin/c203.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c203/bin/c203.win32.trb",
            map: &[
                (1, 4),
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (1, 26),
                (1, 27),
                (1, 28),
                (1, 29),
                (1, 30),
                (0, 23),
                (0, 24),
                (1, 33),
                (0, 25),
                (0, 26),
                (0, 27),
                (1, 37),
                (1, 38),
                (1, 39),
                (1, 40),
            ],
            mesh_renames: &[("c_n909", "c_c203")],
            bone_renames: &[],
            e_ops: &[],
            skel_merge: true,
            skel_s: true,
            face_fix: true,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "vanille",
        costume: "nora",
        targets: &[TargetSwap {
            models: &["chr/npc/n903/bin/n903.win32", "chr/pc/c205/bin/c205.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c205/bin/c205.win32.trb",
            map: &[
                (1, 0),
                (1, 1),
                (1, 2),
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (0, 22),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_n903", "c_c205")],
            bone_renames: &[
                ("F_hair1a", "F_dmhr1a"),
                ("F_hair2a", "F_dmhr2a"),
                ("F_hair3a", "F_dmhr3a"),
                ("F_hair3b", "F_dmhr3b"),
                ("F_hair4a", "F_dmhr4a"),
                ("L_hair2a", "L_dmhr2a"),
                ("R_hair2a", "R_dmhr2a"),
            ],
            e_ops: &[],
            skel_merge: true,
            skel_s: true,
            face_fix: true,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "vanille",
        costume: "deportee",
        targets: &[TargetSwap {
            models: &["chr/pc/c605/bin/c605.win32", "chr/pc/c205/bin/c205.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c205/bin/c205.win32.trb",
            map: &[
                (1, 7),
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (0, 21),
                (0, 22),
                (1, 37),
                (0, 23),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_c605", "c_c205"), ("c20x_puls", "c60x_puls")],
            bone_renames: &[],
            e_ops: &[
                (10, 3, 11),
                (223, 3, -1),
                (31, 3, 172),
                (212, 4, -1),
                (223, 0, 1),
                (223, 13, 0),
                (223, 14, 0),
                (223, 15, 0),
            ],
            skel_merge: true,
            skel_s: false,
            face_fix: false,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "hope",
        costume: "maqui",
        targets: &[TargetSwap {
            models: &["chr/npc/n912/bin/n912.win32", "chr/pc/c204/bin/c204.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c204/bin/c204.win32.trb",
            map: &[
                (1, 0),
                (1, 1),
                (1, 2),
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (0, 20),
                (0, 21),
                (1, 37),
                (0, 22),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_n912", "c_c204")],
            bone_renames: &[],
            e_ops: &[],
            skel_merge: true,
            skel_s: true,
            face_fix: true,
            rename_exact: false,
        }],
    },
    Recipe {
        character: "hope",
        costume: "deportee",
        targets: &[TargetSwap {
            models: &["chr/pc/c604/bin/c604.win32", "chr/pc/c204/bin/c204.win32"],
            base_idx: 1,
            out_rel: "chr/pc/c204/bin/c204.win32.trb",
            map: &[
                (1, 0),
                (1, 1),
                (1, 2),
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (1, 30),
                (1, 31),
                (1, 32),
                (1, 33),
                (1, 34),
                (0, 23),
                (0, 24),
                (1, 39),
                (1, 40),
                (1, 41),
                (1, 42),
                (1, 43),
            ],
            mesh_renames: &[("c_c604", "c_c204"), ("c20x_puls", "c60x_puls")],
            bone_renames: &[],
            e_ops: &[(211, 0, 1), (211, 13, 0), (211, 14, 0), (211, 15, 0)],
            skel_merge: true,
            skel_s: true,
            face_fix: false,
            rename_exact: false,
        }],
    },
];

/// `n9xx` and `f0xx` live under `chr/npc`, everything else under `chr/pc`.
fn model_rel_path(code: &str) -> String {
    let dir = if code.starts_with('n') || code.starts_with('f') {
        "npc"
    } else {
        "pc"
    };
    format!("chr/{dir}/{code}/bin/{code}.win32")
}

fn replace_in_place(buf: &mut [u8], from: &[u8], to: &[u8]) {
    if from.is_empty() || from.len() != to.len() {
        return;
    }
    let mut i = 0;
    while i + from.len() <= buf.len() {
        if &buf[i..i + from.len()] == from {
            buf[i..i + from.len()].copy_from_slice(to);
            i += from.len();
        } else {
            i += 1;
        }
    }
}

/// Combines the recipe's source models, then folds the target's rig into the donor-rigged
/// skeleton. The returned bytes are ready to drop into the `mods/` tree.
pub fn build_recipe_target(t: &TargetSwap, white_data: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let loaded = load_models(white_data, t.models)?;
    let trbs: Vec<Trb> = loaded
        .iter()
        .map(|(t, _)| Trb::parse(t))
        .collect::<Result<_, _>>()?;
    let sources: Vec<(&Trb, &[u8])> = trbs
        .iter()
        .zip(&loaded)
        .map(|(t, (_, i))| (t, i.as_slice()))
        .collect();

    let map: Vec<(usize, usize)> = t
        .map
        .iter()
        .map(|&(a, b)| (a as usize, b as usize))
        .collect();
    let renames: Vec<(Vec<u8>, Vec<u8>)> = t
        .mesh_renames
        .iter()
        .map(|(a, b)| (a.as_bytes().to_vec(), b.as_bytes().to_vec()))
        .collect();

    let (src_skl, target_phb) = skeleton_and_phb(&trbs, &map, t.base_idx);

    let (mut result, mut imgb) = Trb::combine_model(&trbs[t.base_idx], &sources, &map, &renames)
        .ok_or_else(|| anyhow!("combine failed (unexpected model layout)"))?;

    let bone_renames: Vec<(String, String)> = t
        .bone_renames
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let e_ops: Vec<(usize, u8, i32)> = t
        .e_ops
        .iter()
        .map(|&(a, b, c)| (a as usize, b, c))
        .collect();

    if t.skel_merge
        && let Some(src_skl) = &src_skl
    {
        result = merge_skeleton(
            &result,
            src_skl,
            &bone_renames,
            &e_ops,
            t.face_fix,
            t.rename_exact,
            t.skel_s,
        )?;
    }
    if let Some(src_skl) = &src_skl {
        rename_phb_bones(
            &mut result,
            src_skl,
            &bone_renames,
            t.rename_exact,
            &target_phb,
        )?;
    }

    pad16(&mut imgb);
    Ok((result, imgb))
}

/// Drops the donor into the target slot, renaming its model-code refs. Animations bind by bone
/// name, so this only works for rigs that share bone names.
pub fn build_generic_swap(
    donor_code: &str,
    target_code: &str,
    white_data: &Path,
) -> Result<(String, Vec<u8>, Vec<u8>)> {
    let donor_rel = model_rel_path(donor_code);
    let mut trb = std::fs::read(white_data.join(format!("{donor_rel}.trb")))
        .map_err(|e| anyhow!("donor model {donor_code}: {e}"))?;
    let mut imgb = std::fs::read(white_data.join(format!("{donor_rel}.imgb"))).unwrap_or_default();

    let from = format!("c_{donor_code}");
    let to = format!("c_{target_code}");
    if from.len() == to.len() {
        replace_in_place(&mut trb, from.as_bytes(), to.as_bytes());
    }

    pad16(&mut imgb);
    let out_rel = format!("chr/pc/{target_code}/bin/{target_code}.win32.trb");
    Ok((out_rel, trb, imgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_table_is_well_formed() {
        for r in RECIPES {
            assert!(
                !r.targets.is_empty(),
                "{}/{}: no targets",
                r.character,
                r.costume
            );
            for t in r.targets {
                assert!(
                    t.base_idx < t.models.len(),
                    "{}/{}: base_idx OOB",
                    r.character,
                    r.costume
                );
                let base = t.models[t.base_idx];
                assert!(
                    t.out_rel.starts_with(base),
                    "{}/{}: out_rel {} does not extend base {base}",
                    r.character,
                    r.costume,
                    t.out_rel
                );
                assert!(t.out_rel.ends_with(".trb"));
                for &(m, _) in t.map {
                    assert!(
                        (m as usize) < t.models.len(),
                        "{}/{}: map model index OOB",
                        r.character,
                        r.costume
                    );
                }
            }
            assert!(
                character_codes(r.character).is_some(),
                "{}: no character codes",
                r.character
            );
        }
    }

    #[test]
    fn lookups_are_case_insensitive() {
        assert!(find("Lightning", "LeBreau").is_some());
        assert!(find("lightning", "hq").is_some());
        assert!(find("nobody", "x").is_none());
        assert_eq!(character_codes("VANILLE"), Some(&["c005", "c205"][..]));
    }
}
