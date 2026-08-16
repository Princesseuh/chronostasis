//! The HD-model packs' `.bin` bundle: named source-mesh entries of high-poly control points.

use byteorder::{ByteOrder, LittleEndian as LE};

const NAME_LEN: usize = 32;
const RECORD_LEN: usize = NAME_LEN + 8;
/// Bytes per control vertex: `(pos vec3, morph vec3)` f32 pair.
pub const VERTEX_LEN: usize = 24;

/// One control vertex: `pos` matched against the existing mesh, `morph` the
/// high-poly target written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlVertex {
    pub pos: [f32; 3],
    pub morph: [f32; 3],
}

/// One named entry's source-mesh bytes.
pub struct Entry<'a> {
    pub name: String,
    /// Vertex count (table `@+0x24`).
    pub vert_count: usize,
    /// Raw bytes (`vert_count*24`).
    pub data: &'a [u8],
}

impl Entry<'_> {
    /// Control vertices `(pos, morph)` in file order.
    pub fn vertices(&self) -> Vec<ControlVertex> {
        (0..self.vert_count)
            .filter_map(|i| {
                let o = i * VERTEX_LEN;
                let r = self.data.get(o..o + VERTEX_LEN)?;
                Some(ControlVertex {
                    pos: rd3(r, 0),
                    morph: rd3(r, 12),
                })
            })
            .collect()
    }
}

fn rd3(d: &[u8], o: usize) -> [f32; 3] {
    [
        f32::from_le_bytes(d[o..o + 4].try_into().unwrap()),
        f32::from_le_bytes(d[o + 4..o + 8].try_into().unwrap()),
        f32::from_le_bytes(d[o + 8..o + 12].try_into().unwrap()),
    ]
}

/// Parses a model bundle into entries (table order); records whose offset/size
/// fall outside the file are skipped.
pub fn entries(bundle: &[u8]) -> Vec<Entry<'_>> {
    let mut out = Vec::new();
    if bundle.len() < 4 {
        return out;
    }
    let count = LE::read_u32(&bundle[0..4]) as usize;
    for i in 0..count {
        let rec = 4 + i * RECORD_LEN;
        let Some(slot) = bundle.get(rec..rec + RECORD_LEN) else {
            break;
        };
        let name = String::from_utf8_lossy(&slot[..NAME_LEN])
            .trim_end_matches('\0')
            .to_string();
        let off = LE::read_u32(&slot[NAME_LEN..]) as usize;
        let vert_count = LE::read_u32(&slot[NAME_LEN + 4..]) as usize;
        let bytes = vert_count * VERTEX_LEN;
        if let Some(data) = bundle.get(off..off.saturating_add(bytes)) {
            out.push(Entry {
                name,
                vert_count,
                data,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_table_and_vertices() {
        // Two entries; data after the 2×40-byte table.
        let data_start = 4 + 2 * RECORD_LEN;
        let mut b = Vec::new();
        b.extend_from_slice(&2u32.to_le_bytes());
        let mut rec = |name: &str, off: u32, vc: u32| {
            let mut field = vec![0u8; NAME_LEN];
            field[..name.len()].copy_from_slice(name.as_bytes());
            b.extend_from_slice(&field);
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&vc.to_le_bytes());
        };
        rec("X_c001_Body.txt", data_start as u32, 1);
        rec("X_c001_Hair1.txt", data_start as u32 + VERTEX_LEN as u32, 1);
        // v0: pos/morph (1,2,3); v1: pos (4,5,6) morph (7,8,9)
        for f in [
            1.0f32, 2.0, 3.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0,
        ] {
            b.extend_from_slice(&f.to_le_bytes());
        }

        let e = entries(&b);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "X_c001_Body.txt");
        assert_eq!(e[0].vert_count, 1);
        assert_eq!(
            e[0].vertices()[0],
            ControlVertex {
                pos: [1.0, 2.0, 3.0],
                morph: [1.0, 2.0, 3.0]
            }
        );
        assert_eq!(
            e[1].vertices()[0],
            ControlVertex {
                pos: [4.0, 5.0, 6.0],
                morph: [7.0, 8.0, 9.0]
            }
        );
    }

    // `@+0x24` is a vertex count not a byte length; entries pack gapless in
    // offset order (`offset+vert_count*24` == next offset). Set FF13_MODEL_BUNDLE.
    #[test]
    fn real_bundle_is_vertcount_packed_gapless() {
        let Ok(path) = std::env::var("FF13_MODEL_BUNDLE") else {
            eprintln!("skipping: set FF13_MODEL_BUNDLE to a model bundle .bin");
            return;
        };
        let b = std::fs::read(&path).unwrap();
        let count = LE::read_u32(&b[0..4]) as usize;
        let mut offs: Vec<(usize, usize)> = (0..count)
            .map(|i| {
                let rec = 4 + i * RECORD_LEN;
                (
                    LE::read_u32(&b[rec + NAME_LEN..]) as usize,
                    LE::read_u32(&b[rec + NAME_LEN + 4..]) as usize, // vert count
                )
            })
            .collect();
        offs.sort();
        for w in offs.windows(2) {
            assert_eq!(
                w[0].0 + w[0].1 * VERTEX_LEN,
                w[1].0,
                "entry at off {} ({} verts) does not pack gapless into next at {}",
                w[0].0,
                w[0].1,
                w[1].0
            );
        }
        // each entry decodes to exactly vert_count records
        for e in entries(&b) {
            assert_eq!(e.vertices().len(), e.vert_count, "{}", e.name);
        }
    }
}
