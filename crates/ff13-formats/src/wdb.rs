//! WDB Crystal Tools database, converted to and from editable JSON.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::{FormatError, Result, Wpd, WpdEntry};

const STRING: &str = "!!string";
const STRTYPELIST: &str = "!!strtypelist";
const TYPELIST: &str = "!!typelist";
const VERSION: &str = "!!version";

/// `name` is the base filename, e.g. `white`, and selects the known-sheet field list if there is one.
pub fn decode(wdb: &[u8], name: Option<&str>) -> Result<String> {
    serde_json::to_string_pretty(&to_json(wdb, name)?).map_err(json_err)
}

/// Named versus generic is detected from the JSON itself.
pub fn encode(json_text: &str) -> Result<Vec<u8>> {
    from_json(&serde_json::from_str(json_text).map_err(json_err)?)
}

fn known_fields(name: &str) -> Option<&'static [&'static str]> {
    let sheet = crate::wdb_dicts::RECORD_IDS
        .iter()
        .find(|(k, _)| *k == name)?
        .1;
    crate::wdb_dicts::FIELD_NAMES
        .iter()
        .find(|(k, _)| *k == sheet)
        .map(|(_, v)| *v)
}

pub fn to_json(wdb: &[u8], name: Option<&str>) -> Result<Value> {
    let wpd = Wpd::parse(wdb)?;
    let mut strtypelist: Option<Vec<u32>> = None;
    let mut typelist: Vec<u32> = Vec::new();
    let mut version: u32 = 0;
    let mut strings: &[u8] = &[];
    let mut records: Vec<&WpdEntry> = Vec::new();
    for e in &wpd.entries {
        match e.name.as_str() {
            STRING => strings = &e.data,
            STRTYPELIST => strtypelist = Some(u32_be_vec(&e.data)),
            TYPELIST => typelist = u32_be_vec(&e.data),
            VERSION => version = read_u32(&e.data, 0)?,
            "!!sheetname" | "!!strArray" => {
                return Err(FormatError::Unsupported(
                    "XIII-2 / LR WDB (sheetname/strArray) not supported".into(),
                ));
            }
            n if n.starts_with('!') => {}
            _ => records.push(e),
        }
    }
    let strtypelist = strtypelist.ok_or_else(|| malformed("missing !!strtypelist"))?;

    if let (Some(n), Some(fields)) = (name, name.and_then(known_fields)) {
        let sheet = crate::wdb_dicts::RECORD_IDS
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, s)| *s)
            .unwrap_or("");
        return to_json_named(
            &records,
            &strtypelist,
            &typelist,
            version,
            strings,
            fields,
            sheet,
        );
    }

    let mut recs = Vec::with_capacity(records.len());
    for e in &records {
        let mut obj = Map::new();
        obj.insert("record".into(), json!(e.name));
        let mut pos = 0usize;
        let mut counters = [0u32; 4];
        for &t in &strtypelist {
            match t {
                0 => {
                    let v = read_u32(&e.data, pos)?;
                    obj.insert(
                        field("bitpacked-field", &mut counters[0]),
                        json!(format!("0x{v:08X}")),
                    );
                    pos += 4;
                }
                1 => {
                    let f = f32::from_be_bytes(slice4(&e.data, pos)?);
                    obj.insert(field("float-field", &mut counters[1]), json!(f));
                    pos += 4;
                }
                2 => {
                    let off = read_u32(&e.data, pos)? as usize;
                    obj.insert(
                        field("!!string-field", &mut counters[2]),
                        json!(cstr(strings, off)?),
                    );
                    pos += 4;
                }
                3 => {
                    let u = read_u32(&e.data, pos)?;
                    obj.insert(field("uint-field", &mut counters[3]), json!(u));
                    pos += 4;
                }
                other => return Err(malformed(&format!("unknown strtype {other}"))),
            }
        }
        recs.push(Value::Object(obj));
    }

    Ok(json!({
        "recordCount": records.len(),
        "isKnown": false,
        STRTYPELIST: strtypelist,
        TYPELIST: typelist,
        VERSION: version,
        "records": recs,
    }))
}

fn to_json_named(
    records: &[&WpdEntry],
    strtypelist: &[u32],
    typelist: &[u32],
    version: u32,
    strings: &[u8],
    fields: &[&str],
    sheet: &str,
) -> Result<Value> {
    let mut recs = Vec::with_capacity(records.len());
    for e in records {
        let mut obj = Map::new();
        obj.insert("record".into(), json!(e.name));
        decode_record_named(&e.data, strtypelist, fields, strings, &mut obj)?;
        recs.push(Value::Object(obj));
    }
    Ok(json!({
        "recordCount": records.len(),
        "isKnown": true,
        "!!sheetname": sheet,
        STRTYPELIST: strtypelist,
        TYPELIST: typelist,
        VERSION: version,
        "!structitem": fields,
        "records": recs,
    }))
}

/// Expands bit-packed 32-bit words per the field-name list.
fn decode_record_named(
    blob: &[u8],
    strtypelist: &[u32],
    fields: &[&str],
    strings: &[u8],
    obj: &mut Map<String, Value>,
) -> Result<()> {
    let fc = fields.len();
    let (mut num2, mut pos, mut j) = (0usize, 0usize, 0usize);
    while j < fc {
        match *strtypelist
            .get(num2)
            .ok_or_else(|| malformed("strtype underrun"))?
        {
            0 => {
                let word = read_u32(blob, pos)?;
                let (mut num6, mut num7) = (32i32, 32i32);
                while num7 != 0 && j < fc {
                    let f = fields[j];
                    let t = f.as_bytes().first().copied().unwrap_or(b'u');
                    let w = derive_width(f) as i32;
                    if w == 0 {
                        if num6 != 32 {
                            return Err(malformed("full-width field inside a partial word"));
                        }
                        obj.insert(f.to_string(), bit_value(t, word, 0, 32));
                        num7 = 0;
                    } else if w > num7 {
                        j = j.wrapping_sub(1);
                        num7 = 0;
                    } else {
                        num6 -= w;
                        obj.insert(f.to_string(), bit_value(t, word, num6 as usize, w as usize));
                        num7 -= w;
                        if num7 != 0 {
                            j += 1;
                        }
                    }
                }
                num2 += 1;
                pos += 4;
            }
            1 => {
                obj.insert(
                    fields[j].to_string(),
                    json!(f32::from_be_bytes(slice4(blob, pos)?)),
                );
                num2 += 1;
                pos += 4;
            }
            2 => {
                let off = read_u32(blob, pos)? as usize;
                obj.insert(fields[j].to_string(), json!(cstr(strings, off)?));
                num2 += 1;
                pos += 4;
            }
            3 => {
                if fields[j].starts_with("u64") {
                    obj.insert(
                        fields[j].to_string(),
                        json!(u64::from_be_bytes(slice8(blob, pos)?)),
                    );
                    num2 += 1;
                    pos += 8;
                } else {
                    obj.insert(fields[j].to_string(), json!(read_u32(blob, pos)?));
                    num2 += 1;
                    pos += 4;
                }
            }
            other => return Err(malformed(&format!("unknown strtype {other}"))),
        }
        j += 1;
    }
    Ok(())
}

/// `start` counts from the MSB.
fn bit_value(ty: u8, word: u32, start: usize, count: usize) -> Value {
    let u = if count >= 32 {
        word
    } else {
        (word >> (32 - start - count)) & ((1u32 << count) - 1)
    };
    if ty == b'u' {
        json!(u)
    } else {
        let v = if count < 32 && (u >> (count - 1)) & 1 == 1 {
            (u as i64 - (1i64 << count)) as i32
        } else {
            u as i32
        };
        json!(v)
    }
}

/// Read from the digits after the type letter: `i31FileId` is 31 bits, `fVal` a full field.
fn derive_width(name: &str) -> u32 {
    let b = name.as_bytes();
    if b.len() < 2 || !b[1].is_ascii_digit() {
        return 0;
    }
    let mut s = String::from(b[1] as char);
    if b.len() >= 3 && b[2].is_ascii_digit() {
        s.push(b[2] as char);
    }
    s.parse().unwrap_or(0)
}

fn slice8(buf: &[u8], at: usize) -> Result<[u8; 8]> {
    buf.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| malformed("blob too short"))
}

pub fn from_json(v: &Value) -> Result<Vec<u8>> {
    let strtypelist = json_u32_vec(&v[STRTYPELIST])?;
    let typelist = json_u32_vec(&v[TYPELIST])?;
    let version = v[VERSION]
        .as_u64()
        .ok_or_else(|| malformed("bad !!version"))? as u32;
    let records = v["records"]
        .as_array()
        .ok_or_else(|| malformed("missing records"))?;
    let named = v["isKnown"].as_bool() == Some(true) && v.get("!structitem").is_some();
    let fields: Vec<&str> = if named {
        v["!structitem"]
            .as_array()
            .ok_or_else(|| malformed("bad !structitem"))?
            .iter()
            .filter_map(Value::as_str)
            .collect()
    } else {
        Vec::new()
    };

    // First-seen order is what reproduces the shipped files byte for byte.
    let mut pool_order: Vec<String> = vec![String::new()];
    let mut pool_map: HashMap<String, u32> = HashMap::from([(String::new(), 0u32)]);
    let mut next: u32 = 1;

    let mut rec_entries: Vec<WpdEntry> = Vec::with_capacity(records.len());
    for rec in records {
        let obj = rec
            .as_object()
            .ok_or_else(|| malformed("record is not an object"))?;
        let name = obj["record"]
            .as_str()
            .ok_or_else(|| malformed("record missing name"))?;
        let blob = if named {
            encode_record_named(
                obj,
                &strtypelist,
                &fields,
                &mut pool_map,
                &mut pool_order,
                &mut next,
            )?
        } else {
            encode_record_generic(obj, &strtypelist, &mut pool_map, &mut pool_order, &mut next)?
        };
        rec_entries.push(WpdEntry {
            name: name.to_string(),
            ext: String::new(),
            data: blob,
        });
    }

    let mut pool = Vec::new();
    for s in &pool_order {
        pool.extend_from_slice(s.as_bytes());
        pool.push(0);
    }

    let mut entries = vec![
        WpdEntry {
            name: STRING.into(),
            ext: String::new(),
            data: pool,
        },
        WpdEntry {
            name: STRTYPELIST.into(),
            ext: String::new(),
            data: u32_be_bytes(&strtypelist),
        },
        WpdEntry {
            name: TYPELIST.into(),
            ext: String::new(),
            data: u32_be_bytes(&typelist),
        },
        WpdEntry {
            name: VERSION.into(),
            ext: String::new(),
            data: version.to_be_bytes().to_vec(),
        },
    ];
    entries.extend(rec_entries);
    Ok(Wpd { entries }.write())
}

fn encode_record_generic(
    obj: &Map<String, Value>,
    strtypelist: &[u32],
    map: &mut HashMap<String, u32>,
    order: &mut Vec<String>,
    next: &mut u32,
) -> Result<Vec<u8>> {
    let mut blob = Vec::with_capacity(strtypelist.len() * 4);
    let mut c = [0u32; 4];
    for &t in strtypelist {
        match t {
            0 => {
                let s = get_str(obj, &field("bitpacked-field", &mut c[0]))?;
                let u = u32::from_str_radix(s.trim_start_matches("0x"), 16)
                    .map_err(|_| malformed(&format!("bad bitpacked hex {s:?}")))?;
                blob.extend_from_slice(&u.to_be_bytes());
            }
            1 => {
                let key = field("float-field", &mut c[1]);
                let f = obj[&key]
                    .as_f64()
                    .ok_or_else(|| malformed(&format!("bad float {key}")))?
                    as f32;
                blob.extend_from_slice(&f.to_be_bytes());
            }
            2 => {
                let s = get_str(obj, &field("!!string-field", &mut c[2]))?.to_string();
                blob.extend_from_slice(&intern(map, order, next, &s).to_be_bytes());
            }
            3 => {
                let key = field("uint-field", &mut c[3]);
                let u = obj[&key]
                    .as_u64()
                    .ok_or_else(|| malformed(&format!("bad uint {key}")))?
                    as u32;
                blob.extend_from_slice(&u.to_be_bytes());
            }
            other => return Err(malformed(&format!("unknown strtype {other}"))),
        }
    }
    Ok(blob)
}

/// Inverse of [`decode_record_named`].
fn encode_record_named(
    obj: &Map<String, Value>,
    strtypelist: &[u32],
    fields: &[&str],
    map: &mut HashMap<String, u32>,
    order: &mut Vec<String>,
    next: &mut u32,
) -> Result<Vec<u8>> {
    let fc = fields.len();
    let mut blob = Vec::new();
    let (mut num4, mut j) = (0usize, 0usize);
    while j < fc {
        match *strtypelist
            .get(num4)
            .ok_or_else(|| malformed("strtype underrun"))?
        {
            0 => {
                let mut word = 0u32;
                let mut shift = 0u32;
                let mut num5 = 32i32;
                while num5 != 0 && j < fc {
                    let f = fields[j];
                    let t = f.as_bytes().first().copied().unwrap_or(b'u');
                    let mut w = derive_width(f) as i32;
                    if w == 0 {
                        w = 32;
                    }
                    if w > num5 {
                        j = j.wrapping_sub(1);
                        break;
                    }
                    let val = if t == b'u' {
                        obj.get(f).and_then(Value::as_u64).unwrap_or(0) as u32
                    } else {
                        obj.get(f).and_then(Value::as_i64).unwrap_or(0) as u32
                    };
                    let masked = if w == 32 {
                        val
                    } else {
                        val & ((1u32 << w) - 1)
                    };
                    word |= masked << shift;
                    shift += w as u32;
                    num5 -= w;
                    if num5 != 0 {
                        j += 1;
                    }
                }
                blob.extend_from_slice(&word.to_be_bytes());
                num4 += 1;
            }
            1 => {
                let val = obj.get(fields[j]).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                blob.extend_from_slice(&val.to_be_bytes());
                num4 += 1;
            }
            2 => {
                let s = obj.get(fields[j]).and_then(Value::as_str).unwrap_or("");
                blob.extend_from_slice(&intern(map, order, next, s).to_be_bytes());
                num4 += 1;
            }
            3 => {
                if fields[j].starts_with("u64") {
                    let val = obj.get(fields[j]).and_then(Value::as_u64).unwrap_or(0);
                    blob.extend_from_slice(&val.to_be_bytes());
                } else {
                    let val = obj.get(fields[j]).and_then(Value::as_u64).unwrap_or(0) as u32;
                    blob.extend_from_slice(&val.to_be_bytes());
                }
                num4 += 1;
            }
            other => return Err(malformed(&format!("unknown strtype {other}"))),
        }
        j += 1;
    }
    Ok(blob)
}

/// Returns the byte offset, which is 0 for an empty string.
fn intern(map: &mut HashMap<String, u32>, order: &mut Vec<String>, next: &mut u32, s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    if let Some(&o) = map.get(s) {
        return o;
    }
    let o = *next;
    map.insert(s.to_string(), o);
    order.push(s.to_string());
    *next += s.len() as u32 + 1;
    o
}

fn field(prefix: &str, counter: &mut u32) -> String {
    let n = *counter;
    *counter += 1;
    format!("{prefix}_{n}")
}

fn get_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(&format!("missing/!string field {key}")))
}

fn u32_be_vec(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
        .collect()
}

fn u32_be_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn json_u32_vec(v: &Value) -> Result<Vec<u32>> {
    v.as_array()
        .ok_or_else(|| malformed("expected array"))?
        .iter()
        .map(|x| {
            x.as_u64()
                .map(|n| n as u32)
                .ok_or_else(|| malformed("expected u32"))
        })
        .collect()
}

fn cstr(buf: &[u8], off: usize) -> Result<String> {
    let s = buf
        .get(off..)
        .ok_or_else(|| malformed("string offset out of range"))?;
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

fn read_u32(buf: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(slice4(buf, at)?))
}
fn slice4(buf: &[u8], at: usize) -> Result<[u8; 4]> {
    buf.get(at..at + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| malformed("record blob too short"))
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "WDB",
        detail: detail.to_string(),
    }
}
fn json_err(e: serde_json::Error) -> FormatError {
    FormatError::Malformed {
        format: "WDB",
        detail: format!("json: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_wdb_json_round_trips() {
        let v = json!({
            VERSION: 0,
            STRTYPELIST: [],
            TYPELIST: [],
            "records": [],
            "isKnown": false,
        });
        let bytes = from_json(&v).expect("from_json");
        let decoded = to_json(&bytes, None).expect("to_json");
        assert_eq!(decoded["records"].as_array().unwrap().len(), 0);
        let bytes2 = from_json(&decoded).expect("re-encode");
        assert_eq!(
            bytes, bytes2,
            "WDB encode must be stable across a decode round-trip"
        );
    }

    #[test]
    fn roundtrip_real_if_present() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let fl = crate::Filelist::read(format!("{dir}/sys/filelistu.win32.bin"), crate::Game::XIII)
            .unwrap();
        let entry = fl
            .entries
            .iter()
            .find(|e| e.path.ends_with(".wdb"))
            .expect("a .wdb in the main archive");
        let mut img = std::fs::File::open(format!("{dir}/sys/white_imgu.win32.bin")).unwrap();
        let wdb = fl.extract(&mut img, entry).unwrap();
        let name = std::path::Path::new(&entry.path)
            .file_name()
            .and_then(|f| f.to_str())
            .map(|f| {
                f.trim_end_matches(".wdb")
                    .trim_end_matches(".win32")
                    .to_string()
            });
        eprintln!("testing {} ({} bytes) name={name:?}", entry.path, wdb.len());
        let json = to_json(&wdb, name.as_deref()).unwrap();
        eprintln!("  isKnown={}", json["isKnown"]);
        let rebuilt = from_json(&json).unwrap();
        let json2 = to_json(&rebuilt, name.as_deref()).unwrap();
        assert_eq!(json, json2, "WDB JSON round-trip mismatch");
        let rec_count = json["records"].as_array().unwrap().len();
        eprintln!(
            "real WDB: {} records, {} strtypes, orig {} bytes, rebuilt {} bytes, byte-identical={}",
            rec_count,
            json[STRTYPELIST].as_array().unwrap().len(),
            wdb.len(),
            rebuilt.len(),
            wdb == rebuilt
        );
    }
}
