//! `SEDBelb` sockets: named attachment points parented to skeleton joints.

use byteorder::{ByteOrder, LittleEndian as LE};

const SOCKET_SIZE: usize = 0x20;

#[derive(Debug, Clone)]
pub struct Socket {
    /// e.g. `"attach_active_weapon_right"`, `"generate_magic_0"`.
    pub name: String,
    /// A joint name from the model's [`crate::skl`] skeleton.
    pub parent: String,
    pub location: [f32; 3],
    /// Euler XYZ, in radians.
    pub rotation: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Sockets {
    pub sockets: Vec<Socket>,
}

impl Sockets {
    /// `res` starts at the `SEDB` section header.
    pub fn parse(section: &[u8]) -> Option<Sockets> {
        if section.len() < 0x40 || &section[0..4] != b"SEDB" || &section[4..7] != b"elb" {
            return None;
        }
        let u32 = |o: usize| -> Option<u32> { section.get(o..o + 4).map(LE::read_u32) };
        let f32 = |o: usize| -> Option<f32> { section.get(o..o + 4).map(LE::read_f32) };
        let cstr = |o: usize| -> String {
            let Some(s) = section.get(o..) else {
                return String::new();
            };
            let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            String::from_utf8_lossy(&s[..end]).into_owned()
        };

        let count = u32(0x30)? as usize;
        let euler_base = 0x40 + count.checked_mul(SOCKET_SIZE)?;
        let mut sockets = Vec::with_capacity(count);
        for s in 0..count {
            let e = 0x40 + s * SOCKET_SIZE;
            let r = euler_base + s * 0xC;
            sockets.push(Socket {
                parent: cstr(u32(e)? as usize),
                name: cstr(u32(e + 4)? as usize),
                location: [f32(e + 8)?, f32(e + 12)?, f32(e + 16)?],
                rotation: [f32(r)?, f32(r + 4)?, f32(r + 8)?],
            });
        }
        Some(Sockets { sockets })
    }
}

#[cfg(test)]
mod tests {
    use crate::trb::Trb;

    #[test]
    fn sockets_parse() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (mut tables, mut sockets) = (0u64, 0u64);
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
                let Some(elb) = trb.sockets() else { continue };
                tables += 1;
                for s in &elb.sockets {
                    sockets += 1;
                    assert!(!s.name.is_empty(), "empty socket name in {}", p.display());
                    assert!(
                        !s.parent.is_empty(),
                        "empty socket parent in {}",
                        p.display()
                    );
                    assert!(
                        s.location.iter().chain(&s.rotation).all(|v| v.is_finite()),
                        "non-finite socket transform in {}",
                        p.display()
                    );
                }
            }
        }
        eprintln!("parsed {tables} socket tables, {sockets} sockets");
        assert!(
            tables > 0,
            "no SEDBelb socket tables found under FF13_MODELS_DIR"
        );
    }
}
