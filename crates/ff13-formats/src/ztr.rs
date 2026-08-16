//! ZTR keyed compressed text, to and from readable `key |:| text` lines.

use std::collections::HashMap;
use std::sync::OnceLock;

use byteorder::{BigEndian as BE, ByteOrder};

use crate::{FormatError, Game, Result, ztr_dicts};

const SEP: &[u8] = b" |:| ";

type PairMap = HashMap<(u8, u8), &'static str>;

struct Dicts {
    color: PairMap,
    icon: PairMap,
    btn: PairMap,
    single: HashMap<u8, &'static str>,
    base_chara: PairMap,
    special: PairMap,
    ex_chara: PairMap,
    unk: PairMap,
    unk2: PairMap,
    decoded: HashMap<&'static str, &'static str>,
}

fn dicts() -> &'static Dicts {
    static D: OnceLock<Dicts> = OnceLock::new();
    D.get_or_init(|| {
        let pair =
            |s: &'static [((u8, u8), &'static str)]| -> PairMap { s.iter().copied().collect() };
        Dicts {
            color: pair(ztr_dicts::COLOR_KEYS),
            icon: pair(ztr_dicts::ICON_KEYS),
            btn: pair(ztr_dicts::BTN_KEYS),
            single: ztr_dicts::SINGLE_KEYS.iter().copied().collect(),
            base_chara: pair(ztr_dicts::BASE_CHARA_KEYS),
            special: pair(ztr_dicts::SPECIAL_KEYS),
            ex_chara: pair(ztr_dicts::EX_CHARA_KEYS),
            unk: pair(ztr_dicts::UNK_KEYS),
            unk2: pair(ztr_dicts::UNK2_KEYS),
            decoded: ztr_dicts::DECODED_CHARA_KEYS.iter().copied().collect(),
        }
    })
}

pub fn decode(ztr: &[u8], game: Game) -> Result<String> {
    if game != Game::XIII {
        return Err(FormatError::Unsupported(
            "ZTR only supports FFXIII for now".into(),
        ));
    }
    if ztr.len() < 20 {
        return Err(malformed("file too small"));
    }

    let line_count = BE::read_u32(&ztr[8..12]) as usize;
    let dcmp_ids_size = BE::read_u32(&ztr[12..16]) as usize;
    let dict_offsets_count = BE::read_u32(&ztr[16..20]) as usize;

    let after_header = 20;
    let line_info_table = after_header + dict_offsets_count * 4;
    let mut dict_offsets = Vec::with_capacity(dict_offsets_count.min(ztr.len() / 4));
    for i in 0..dict_offsets_count {
        dict_offsets.push(BE::read_u32(get(ztr, after_header + i * 4, 4)?) as usize);
    }
    let ids_start = line_info_table + line_count * 4;

    // The size field is untrusted, so cap the capacity hint.
    let mut ids = Vec::with_capacity(dcmp_ids_size.min(ztr.len().saturating_mul(16)));
    let mut pos = ids_start;
    for group in group_sizes(dcmp_ids_size) {
        let chunk_size = BE::read_u32(get(ztr, pos, 4)?) as usize;
        let pages = build_pages(ztr, pos + 4, chunk_size)?;
        pos += 4 + chunk_size;
        let mut remaining = group;
        while remaining > 0 {
            let b = *get1(ztr, pos)?;
            pos += 1;
            match pages.get(&b) {
                Some(exp) => {
                    for &x in exp {
                        ids.push(x);
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                None => {
                    ids.push(b);
                    remaining -= 1;
                }
            }
        }
    }

    let line_data = ztr
        .get(pos..)
        .ok_or_else(|| malformed("line-data section starts past end of file"))?;
    let d = dicts();
    let mut stream: Vec<u8> = Vec::new();
    let mut id_pos = 0usize;
    let mut prev_chunk_id: i32 = 0;
    let mut need_setup = true;
    let mut data_start = 0usize;
    let mut chunk_dict: HashMap<u8, Vec<u8>> = HashMap::new();

    for line in 0..line_count {
        let row = get(ztr, line_info_table + line * 4, 4)?;
        let mut chunk_id = row[0] as usize;
        let mut chara_start = row[1] as usize;
        let line_start = BE::read_u16(&row[2..4]) as usize;

        while let Some(&b) = ids.get(id_pos) {
            id_pos += 1;
            if b == 0 {
                break;
            }
            stream.push(b);
        }
        stream.extend_from_slice(SEP);

        if chunk_id as i32 != prev_chunk_id {
            need_setup = true;
        }
        prev_chunk_id = chunk_id as i32;
        if need_setup {
            let dp = *dict_offsets
                .get(chunk_id)
                .ok_or_else(|| malformed("line references a missing dictionary chunk"))?;
            let cs = BE::read_u32(get(line_data, dp, 4)?) as usize;
            chunk_dict = build_pages(line_data, dp + 4, cs)?;
            data_start = dp + 4 + cs;
            need_setup = false;
        }

        let mut p = data_start + line_start;
        let mut b2: u8 = 0xFF;
        let mut b3: u8 = 0xFF;
        loop {
            if dict_offsets.get(chunk_id + 1) == Some(&p) {
                chunk_id += 1;
                let dp = dict_offsets[chunk_id];
                let cs = BE::read_u32(get(line_data, dp, 4)?) as usize;
                chunk_dict = build_pages(line_data, dp + 4, cs)?;
                data_start = dp + 4 + cs;
                p = data_start;
            }
            let mut b = *get1(line_data, p)?;
            p += 1;
            if b2 == 0 && b == 0 {
                stream.push(b);
                break;
            }
            b2 = b;
            if let Some(exp) = chunk_dict.get(&b) {
                b2 = b3;
                let mut k = 0usize;
                let mut ended = false;
                while k < exp.len() {
                    k += chara_start;
                    chara_start = 0;
                    if k >= exp.len() {
                        break;
                    }
                    b = exp[k];
                    if b2 == 0 && b == 0 {
                        stream.push(b);
                        ended = true;
                        break;
                    }
                    stream.push(b);
                    b2 = b;
                    k += 1;
                }
                if ended {
                    break;
                }
            } else {
                stream.push(b);
            }
            b3 = b;
        }
        stream.push(13);
        stream.push(10);
    }

    Ok(finalize(&decode_symbols(&stream, d), &d.decoded))
}

/// Both `compress` paths work in-game; uncompressed is larger but simpler.
pub fn encode(txt: &str, game: Game, compress: bool) -> Result<Vec<u8>> {
    if game != Game::XIII {
        return Err(FormatError::Unsupported(
            "ZTR only supports FFXIII for now".into(),
        ));
    }
    let sep = " |:| ";
    let lines: Vec<(&str, &str)> = txt
        .lines()
        .map(|l| l.split_once(sep).unwrap_or((l, "")))
        .collect();
    let line_count = lines.len();

    let mut ids_raw = Vec::new();
    for (k, _) in &lines {
        ids_raw.extend_from_slice(k.as_bytes());
        ids_raw.push(0);
    }
    let dcmp_ids_size = ids_raw.len();
    let processed_ids = pack_ids(&ids_raw, compress);

    let mut unprocessed = Vec::new();
    for (_, t) in &lines {
        unprocessed.extend_from_slice(t.as_bytes());
        unprocessed.push(0);
        unprocessed.push(0);
    }
    let processed_lines = encode_symbols(&unprocessed);

    if compress {
        Ok(build_compressed(
            line_count,
            dcmp_ids_size,
            &processed_ids,
            &processed_lines,
        ))
    } else {
        Ok(build_uncmp(
            line_count,
            dcmp_ids_size,
            &processed_ids,
            &processed_lines,
        ))
    }
}

fn pack_ids(ids: &[u8], compress: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    for g in group_sizes(ids.len()) {
        if compress {
            out.extend_from_slice(&compress_chunk(&ids[pos..pos + g]));
        } else {
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&ids[pos..pos + g]);
        }
        pos += g;
    }
    out
}

fn compress_chunk(data: &[u8]) -> Vec<u8> {
    let free: Vec<u8> = {
        let mut present = [false; 256];
        for &b in data {
            present[b as usize] = true;
        }
        (0u16..=255)
            .filter(|&i| i != 8 && !present[i as usize])
            .map(|i| i as u8)
            .collect()
    };
    let mut table: Vec<u8> = Vec::new();
    let mut cur = data.to_vec();
    let mut pi = 0;
    while pi < free.len() {
        let (b1, b2, count) = largest_pair(&cur);
        if count < 4 {
            break;
        }
        let page = free[pi];
        table.extend_from_slice(&[page, b1, b2]);
        let mut out = Vec::with_capacity(cur.len());
        let mut k = 0;
        while k < cur.len() {
            if k + 1 < cur.len() && cur[k] == b1 && cur[k + 1] == b2 {
                out.push(page);
                k += 2;
            } else {
                out.push(cur[k]);
                k += 1;
            }
        }
        cur = out;
        pi += 1;
    }
    let mut res = Vec::with_capacity(4 + table.len() + cur.len());
    res.extend_from_slice(&(table.len() as u32).to_be_bytes());
    res.extend_from_slice(&table);
    res.extend_from_slice(&cur);
    res
}

/// Ties keep the first-seen pair, which is what reproduces the shipped files byte for byte.
fn largest_pair(data: &[u8]) -> (u8, u8, usize) {
    if data.len() < 2 {
        return (0, 0, 0);
    }
    // The stored position is where the next non-overlapping match may start.
    let mut counts: HashMap<(u8, u8), (usize, usize)> = HashMap::new();
    let mut order: Vec<(u8, u8)> = Vec::new();
    for i in 0..data.len() - 1 {
        let pair = (data[i], data[i + 1]);
        let e = counts.entry(pair).or_insert_with(|| {
            order.push(pair);
            (0, 0)
        });
        if i >= e.1 {
            e.0 += 1;
            e.1 = i + 2;
        }
    }
    let mut best = (0u8, 0u8, 0usize);
    for pair in order {
        let count = counts[&pair].0;
        if count > best.2 {
            best = (pair.0, pair.1, count);
        }
    }
    best
}

fn build_uncmp(line_count: usize, dcmp_ids_size: usize, ids: &[u8], lines: &[u8]) -> Vec<u8> {
    let mut offsets: Vec<u32> = vec![0];
    let mut info: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    info.extend_from_slice(&[0, 0]);
    info.extend_from_slice(&0u16.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes());

    let mut dict_chunk_id: u8 = 0;
    let mut num: u32 = 0;
    let mut lines_done = 0usize;
    let mut b2: u8 = 0xFF;
    for &b in lines {
        num += 1;
        data.push(b);
        if b == 0 && b2 == 0 {
            lines_done += 1;
            if lines_done < line_count {
                info.push(dict_chunk_id);
                info.push(0);
                info.extend_from_slice(&(num as u16).to_be_bytes());
                b2 = 0xFF;
            }
        } else {
            b2 = b;
        }
        if num == 4096 {
            num = 0;
            dict_chunk_id = dict_chunk_id.wrapping_add(1);
            offsets.push(data.len() as u32);
            data.extend_from_slice(&0u32.to_be_bytes());
        }
    }
    offsets.push(data.len() as u32);

    let mut out = Vec::new();
    out.extend_from_slice(&1u64.to_be_bytes());
    out.extend_from_slice(&(line_count as u32).to_be_bytes());
    out.extend_from_slice(&(dcmp_ids_size as u32).to_be_bytes());
    out.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for o in &offsets {
        out.extend_from_slice(&o.to_be_bytes());
    }
    out.extend_from_slice(&info);
    out.extend_from_slice(ids);
    out.extend_from_slice(&data);
    out
}

fn build_compressed(line_count: usize, dcmp_ids_size: usize, ids: &[u8], lines: &[u8]) -> Vec<u8> {
    let mut offsets: Vec<u32> = vec![0];
    let mut line_info: Vec<u8> = Vec::new();
    let mut line_data: Vec<u8> = Vec::new();
    let (mut chunk_id, mut chara_start, mut line_start) = (0u8, 0u8, 0u16);
    let mut flag = true;
    let (mut b2, mut b3) = (0xFFu8, 0xFFu8);
    let mut src = 0usize;
    let mut num = 0usize;

    for g in group_sizes(lines.len()) {
        line_data.extend_from_slice(&compress_chunk(&lines[src..src + g]));
        src += g;
        let chunk_size = u32::from_be_bytes(line_data[num..num + 4].try_into().unwrap()) as usize;
        let dict = build_pages(&line_data, num + 4, chunk_size).unwrap_or_default();
        let mut p = num + 4 + chunk_size;
        while p != line_data.len() {
            if flag {
                line_info.push(chunk_id);
                line_info.push(chara_start);
                line_info.extend_from_slice(&line_start.to_be_bytes());
                flag = false;
                b2 = 0xFF;
                b3 = 0xFF;
            }
            let mut b = line_data[p];
            p += 1;
            if b == 0 && b2 == 0 {
                flag = true;
                line_start = line_start.wrapping_add(1);
                continue;
            }
            b2 = b;
            if let Some(exp) = dict.get(&b) {
                b2 = b3;
                let count = exp.len();
                let mut j = chara_start as usize;
                while j < count {
                    b = exp[j];
                    if b == 0 && b2 == 0 {
                        if j + 1 < count {
                            chara_start = (j + 1) as u8;
                            p -= 1;
                            line_start = line_start.wrapping_sub(1);
                        } else {
                            chara_start = 0;
                        }
                        flag = true;
                        break;
                    }
                    b2 = b;
                    j += 1;
                }
                line_start = line_start.wrapping_add(1);
                if !flag {
                    chara_start = 0;
                }
            } else {
                line_start = line_start.wrapping_add(1);
            }
            b3 = b;
        }
        offsets.push(line_data.len() as u32);
        chunk_id += 1;
        line_start = 0;
        num = line_data.len();
    }

    let mut out = Vec::new();
    out.extend_from_slice(&1u64.to_be_bytes());
    out.extend_from_slice(&(line_count as u32).to_be_bytes());
    out.extend_from_slice(&(dcmp_ids_size as u32).to_be_bytes());
    out.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for o in &offsets {
        out.extend_from_slice(&o.to_be_bytes());
    }
    out.extend_from_slice(&line_info);
    out.extend_from_slice(ids);
    out.extend_from_slice(&line_data);
    out
}

struct RevDicts {
    single: HashMap<&'static str, u8>,
    pairs: Vec<HashMap<&'static str, (u8, u8)>>,
    decoded_val_to_key: HashMap<&'static str, &'static str>,
}

fn rev_dicts() -> &'static RevDicts {
    static R: OnceLock<RevDicts> = OnceLock::new();
    R.get_or_init(|| {
        let mut single = HashMap::new();
        for &(b, s) in ztr_dicts::SINGLE_KEYS {
            single.entry(s).or_insert(b);
        }
        let rev_pair = |slice: &'static [((u8, u8), &'static str)]| {
            let mut m = HashMap::new();
            for &(k, s) in slice {
                m.entry(s).or_insert(k);
            }
            m
        };
        let pairs = vec![
            rev_pair(ztr_dicts::COLOR_KEYS),
            rev_pair(ztr_dicts::ICON_KEYS),
            rev_pair(ztr_dicts::BTN_KEYS),
            rev_pair(ztr_dicts::BASE_CHARA_KEYS),
            rev_pair(ztr_dicts::EX_CHARA_KEYS),
            rev_pair(ztr_dicts::SPECIAL_KEYS),
            rev_pair(ztr_dicts::UNK_KEYS),
            rev_pair(ztr_dicts::UNK2_KEYS),
        ];
        let mut decoded_val_to_key = HashMap::new();
        for &(k, v) in ztr_dicts::DECODED_CHARA_KEYS {
            decoded_val_to_key.entry(v).or_insert(k);
        }
        RevDicts {
            single,
            pairs,
            decoded_val_to_key,
        }
    })
}

/// Inverse of [`decode_symbols`].
fn encode_symbols(unprocessed_utf8: &[u8]) -> Vec<u8> {
    let r = rev_dicts();
    let s = String::from_utf8_lossy(unprocessed_utf8);
    let s = reverse_decoded_chara(&s, &r.decoded_val_to_key);
    let (cp932, _, _) = encoding_rs::SHIFT_JIS.encode(&s);
    let mut out = Vec::with_capacity(cp932.len());
    let mut i = 0;
    while i < cp932.len() {
        if cp932[i] == b'{'
            && let Some(close) = cp932[i + 1..].iter().take(34).position(|&c| c == b'}')
            && let Ok(inner) = std::str::from_utf8(&cp932[i + 1..i + 1 + close])
        {
            let token = format!("{{{inner}}}");
            if let Some(&b) = r.single.get(token.as_str()) {
                out.push(b);
                i += close + 2;
                continue;
            }
            if let Some((b1, b2)) = r.pairs.iter().find_map(|m| m.get(token.as_str()).copied()) {
                out.push(b1);
                out.push(b2);
                i += close + 2;
                continue;
            }
            if inner == "FF" {
                out.push(0xFF);
                i += close + 2;
                continue;
            }
        }
        out.push(cp932[i]);
        i += 1;
    }
    out
}

fn reverse_decoded_chara(text: &str, val_to_key: &HashMap<&'static str, &'static str>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{'
            && let Some(close) = chars[i + 1..].iter().take(34).position(|&c| c == '}')
        {
            let token: String = chars[i..i + 1 + close + 1].iter().collect();
            if let Some(&key) = val_to_key.get(token.as_str()) {
                out.push('{');
                out.push_str(key);
                out.push('}');
                i += close + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Resolves key-dictionary symbols to a cp932 stream, then converts that to UTF-8.
fn decode_symbols(stream: &[u8], d: &Dicts) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(stream.len());
    let mut prev: u8 = 0xFF;
    let mut i = 0usize;
    let n = stream.len();
    while i < n {
        let mut b2 = stream[i];
        let mut flag = false;
        if shift_jis_pair(prev, b2) {
            out.push(b2);
            flag = true;
            b2 = 0;
        }
        if !flag && let Some(tok) = d.single.get(&b2) {
            if b2 == 0 && stream.get(i + 1) == Some(&0) {
                flag = true;
                i += 1;
            }
            if !flag {
                out.extend_from_slice(tok.as_bytes());
                flag = true;
                b2 = 0;
            }
        }
        if !flag && let Some(&b3) = stream.get(i + 1) {
            for dict in [
                &d.color,
                &d.icon,
                &d.btn,
                &d.base_chara,
                &d.special,
                &d.ex_chara,
                &d.unk,
                &d.unk2,
            ] {
                if let Some(tok) = dict.get(&(b2, b3)) {
                    out.extend_from_slice(tok.as_bytes());
                    flag = true;
                    b2 = 0;
                    i += 1;
                    break;
                }
            }
        }
        if !flag && b2 == 0xFF {
            out.extend_from_slice(b"{FF}");
            flag = true;
        }
        if !flag {
            out.push(b2);
        }
        prev = b2;
        i += 1;
    }
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&out);
    text.into_owned()
}

/// `FinalizeTxtFile`: replace `{key}` tokens present in the decoded-chara map.
fn finalize(text: &str, decoded: &HashMap<&'static str, &'static str>) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{'
            && let Some(close) = chars[i + 1..].iter().take(33).position(|&c| c == '}')
        {
            let inner: String = chars[i + 1..i + 1 + close].iter().collect();
            if let Some(&val) = decoded.get(inner.as_str()) {
                out.push_str(val);
                i += close + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `pages_at` is the first 3-byte page entry and `chunk_size` the table length.
fn build_pages(buf: &[u8], pages_at: usize, chunk_size: usize) -> Result<HashMap<u8, Vec<u8>>> {
    let count = chunk_size / 3;
    let table = get(buf, pages_at, count * 3)?;
    let mut order = Vec::with_capacity(count);
    let mut raw: HashMap<u8, (u8, u8)> = HashMap::with_capacity(count);
    for i in 0..count {
        let e = &table[i * 3..i * 3 + 3];
        order.push(e[0]);
        raw.insert(e[0], (e[1], e[2]));
    }
    let mut expanded: HashMap<u8, Vec<u8>> = HashMap::with_capacity(count);
    for &page in &order {
        let (b1, b2) = raw[&page];
        let mut v = Vec::new();
        match expanded.get(&b1) {
            Some(e) if raw.contains_key(&b1) => v.extend_from_slice(e),
            _ => v.push(b1),
        }
        match expanded.get(&b2) {
            Some(e) if raw.contains_key(&b2) => v.extend_from_slice(e),
            _ => v.push(b2),
        }
        expanded.insert(page, v);
    }
    Ok(expanded)
}

fn group_sizes(total: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut n = total;
    while n != 0 {
        let g = n.min(4096);
        out.push(g);
        n -= g;
    }
    out
}

fn get(buf: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    buf.get(at..at + len)
        .ok_or_else(|| malformed("read past end"))
}
fn get1(buf: &[u8], at: usize) -> Result<&u8> {
    buf.get(at).ok_or_else(|| malformed("read past end"))
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "ZTR",
        detail: detail.to_string(),
    }
}

/// Shift-JIS lead/trail byte validity.
fn shift_jis_pair(b1: u8, b2: u8) -> bool {
    let in_any = |b: u8, ranges: &[(u8, u8)]| ranges.iter().any(|&(lo, hi)| b >= lo && b <= hi);
    match b1 {
        129 => in_any(
            b2,
            &[
                (64, 126),
                (128, 172),
                (184, 191),
                (200, 206),
                (218, 232),
                (240, 247),
                (252, 252),
            ],
        ),
        130 => in_any(b2, &[(79, 88), (96, 121), (129, 154), (159, 241)]),
        131 => in_any(b2, &[(64, 126), (128, 150), (159, 182), (191, 214)]),
        132 => in_any(b2, &[(64, 96), (112, 126), (128, 145), (159, 190)]),
        135 => in_any(
            b2,
            &[
                (64, 93),
                (95, 117),
                (126, 126),
                (128, 143),
                (147, 148),
                (152, 153),
            ],
        ),
        136 => in_any(b2, &[(159, 252)]),
        152 => in_any(b2, &[(64, 114), (159, 252)]),
        234 => in_any(b2, &[(64, 126), (128, 164)]),
        250 => in_any(b2, &[(64, 73), (85, 87), (92, 126), (128, 252)]),
        251 => in_any(b2, &[(64, 126), (128, 252)]),
        252 => in_any(b2, &[(64, 75)]),
        137..=151 | 153..=159 | 224..=233 => in_any(b2, &[(64, 126), (128, 252)]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Filelist;

    #[test]
    fn decode_rejects_malformed_without_panicking() {
        // A header claiming more dict offsets than the file holds must error, not panic.
        let mut b = vec![0u8; 20];
        b[16..20].copy_from_slice(&100u32.to_be_bytes());
        assert!(decode(&b, Game::XIII).is_err());

        let junk: Vec<u8> = (0..40).map(|i| (i * 37 % 251) as u8).collect();
        assert!(decode(&junk, Game::XIII).is_err());
    }

    #[test]
    fn encode_decode_round_trips() {
        let txt = "lb_item_0001 |:| Potion\r\n\
                   lb_item_0002 |:| Phoenix Down\r\n\
                   lb_msg_0003 |:| Hello, world!\r\n";
        for compress in [false, true] {
            let bytes = encode(txt, Game::XIII, compress).unwrap();
            let back = decode(&bytes, Game::XIII).unwrap();
            assert_eq!(back, txt, "ZTR round-trip failed (compress={compress})");
        }
    }

    #[test]
    fn decode_real_if_present() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        for (fl_name, img_name) in [
            ("filelistu.win32.bin", "white_imgu.win32.bin"),
            ("filelist_scru.win32.bin", "white_scru.win32.bin"),
        ] {
            let fl = Filelist::read(format!("{dir}/sys/{fl_name}"), Game::XIII).unwrap();
            let Some(entry) = fl.entries.iter().find(|e| e.path.ends_with(".ztr")) else {
                continue;
            };
            let mut img = std::fs::File::open(format!("{dir}/sys/{img_name}")).unwrap();
            let bytes = fl.extract(&mut img, entry).unwrap();
            let txt = decode(&bytes, Game::XIII).unwrap();
            eprintln!(
                "=== {} ({} bytes -> {} chars, {} lines) ===",
                entry.path,
                bytes.len(),
                txt.len(),
                txt.lines().count()
            );
            for line in txt.lines().take(12) {
                eprintln!("{line}");
            }
            let uncmp = encode(&txt, Game::XIII, false).unwrap();
            assert_eq!(
                decode(&uncmp, Game::XIII).unwrap(),
                txt,
                "ZTR uncompressed round-trip mismatch"
            );
            let cmp = encode(&txt, Game::XIII, true).unwrap();
            assert_eq!(
                decode(&cmp, Game::XIII).unwrap(),
                txt,
                "ZTR compressed round-trip mismatch"
            );
            eprintln!(
                "ROUND-TRIP OK: orig {} / uncompressed {} / compressed {} bytes, both re-decoded identical",
                bytes.len(),
                uncmp.len(),
                cmp.len()
            );
            return;
        }
        panic!("no .ztr found");
    }
}
