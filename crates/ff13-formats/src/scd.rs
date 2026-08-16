//! SCD audio container: Square's wrapper around a Vorbis or MS-ADPCM stream.

use byteorder::{ByteOrder, LittleEndian as LE};

use crate::{FormatError, Result};

const CODEC_VORBIS: i32 = 6;
const CODEC_MS_ADPCM: i32 = 12;
const SCD_HEADER_STREAM_PTR: usize = 60;
const SCD_HEADER_STREAM_COUNT: usize = 50;

#[derive(Debug, Clone, Copy)]
pub struct ScdInfo {
    pub codec: i32,
    pub encode_type: i16,
    pub stream_size: u32,
    pub channels: u32,
    pub samplerate: u32,
    pub ogg_data_offset: usize,
    pub extra_header_size: usize,
    pub stream_header_offset: usize,
}

/// Entry 0 of the sub-stream table is often a dummy placeholder, so scan for a real one first.
fn first_decodable_stream(scd: &[u8], sh_ptr: usize) -> Result<usize> {
    let count = scd
        .get(SCD_HEADER_STREAM_COUNT..SCD_HEADER_STREAM_COUNT + 2)
        .map(LE::read_u16)
        .unwrap_or(1)
        .clamp(1, 4096) as usize;
    let entry = |k: usize| -> Option<usize> {
        let at = sh_ptr.checked_add(k * 4)?;
        scd.get(at..at.checked_add(4)?)
            .map(|b| LE::read_i32(b) as usize)
    };
    let is_real = |off: usize| {
        off != 0
            && off
                .checked_add(16)
                .and_then(|end| scd.get(off + 12..end))
                .map(LE::read_i32)
                .is_some_and(|c| c != -1)
    };
    let first = entry(0).ok_or_else(|| FormatError::Malformed {
        format: "SCD",
        detail: "stream-offset table out of range".into(),
    })?;
    if is_real(first) {
        return Ok(first);
    }
    Ok((1..count)
        .filter_map(entry)
        .find(|&off| is_real(off))
        .unwrap_or(first))
}

fn parse(scd: &[u8]) -> Result<ScdInfo> {
    let sh_ptr = LE::read_i32(get(scd, SCD_HEADER_STREAM_PTR, 4)?) as usize;
    let num = first_decodable_stream(scd, sh_ptr)?;
    let stream_size = LE::read_u32(get(scd, num, 4)?);
    let channels = LE::read_u32(get(scd, num + 4, 4)?);
    let samplerate = LE::read_u32(get(scd, num + 8, 4)?);
    let codec = LE::read_i32(get(scd, num + 12, 4)?);
    let extra_header_size = LE::read_u32(get(scd, num + 24, 4)?) as usize;
    let num2 = num + 32;
    let encode_type = LE::read_i16(get(scd, num2, 2)?);
    Ok(ScdInfo {
        codec,
        encode_type,
        stream_size,
        channels,
        samplerate,
        ogg_data_offset: num2 + extra_header_size,
        extra_header_size,
        stream_header_offset: num,
    })
}

pub fn info(scd: &[u8]) -> Result<ScdInfo> {
    parse(scd)
}

pub fn to_ogg(scd: &[u8]) -> Result<Vec<u8>> {
    let i = parse(scd)?;
    if i.codec != CODEC_VORBIS {
        return Err(FormatError::Unsupported(format!(
            "SCD codec {} (only Vorbis is supported for ogg extraction)",
            i.codec
        )));
    }
    let data = get(scd, i.ogg_data_offset, i.stream_size as usize)?.to_vec();
    if i.encode_type == 0 {
        Ok(data)
    } else {
        let num = i.stream_header_offset;
        let num2 = num + 32;
        let num3 = num2 + 32;
        let seek_table_size = LE::read_i32(get(scd, num2 + 20, 4)?);
        let header_size = LE::read_i32(get(scd, num2 + 24, 4)?);
        if seek_table_size < 0 || header_size < 0 {
            return Err(FormatError::Malformed {
                format: "SCD",
                detail: "negative header/seek-table size".into(),
            });
        }
        let (seek_table_size, header_size) = (seek_table_size as usize, header_size as usize);
        let key = LE::read_i16(get(scd, num2 + 2, 2)?) as u8;
        let enc = get(scd, num3 + seek_table_size, header_size)?;
        let mut ogg = Vec::with_capacity(header_size + data.len());
        ogg.extend(enc.iter().map(|b| b ^ key));
        ogg.extend_from_slice(&data);
        Ok(ogg)
    }
}

/// Raw (`encode_type` 0) Vorbis only; an obfuscated stream needs a full encoder to rebuild.
pub fn replace_ogg(scd: &[u8], new_ogg: &[u8]) -> Result<Vec<u8>> {
    let i = parse(scd)?;
    if i.codec != CODEC_VORBIS || i.encode_type != 0 {
        return Err(FormatError::Unsupported(
            "replace requires a raw (encode_type 0) Vorbis SCD".into(),
        ));
    }
    if scd.len() < i.ogg_data_offset {
        return Err(FormatError::Malformed {
            format: "SCD",
            detail: "stream data offset past end of file".into(),
        });
    }
    let tail_start = i.ogg_data_offset + i.stream_size as usize;
    let tail = scd.get(tail_start..).unwrap_or(&[]);
    let mut out = Vec::with_capacity(i.ogg_data_offset + new_ogg.len() + tail.len());
    out.extend_from_slice(&scd[..i.ogg_data_offset]);
    out.extend_from_slice(new_ogg);
    out.extend_from_slice(tail);
    let ss = i.stream_header_offset;
    out[ss..ss + 4].copy_from_slice(&(new_ogg.len() as u32).to_le_bytes());
    let total = out.len() as u32;
    out[16..20].copy_from_slice(&total.to_le_bytes());
    Ok(out)
}

/// Returns `(ext, bytes)`: `ogg` for Vorbis, `wav` for MS-ADPCM.
pub fn extract(scd: &[u8]) -> Result<(&'static str, Vec<u8>)> {
    let i = parse(scd)?;
    match i.codec {
        CODEC_VORBIS => Ok(("ogg", to_ogg(scd)?)),
        CODEC_MS_ADPCM => Ok(("wav", to_wav(scd)?)),
        other => Err(FormatError::Unsupported(format!("SCD codec {other}"))),
    }
}

/// The replacement must match the SCD's own codec.
pub fn replace(scd: &[u8], audio: &[u8]) -> Result<Vec<u8>> {
    let i = parse(scd)?;
    match i.codec {
        CODEC_VORBIS => replace_ogg(scd, audio),
        CODEC_MS_ADPCM => replace_wav(scd, audio),
        other => Err(FormatError::Unsupported(format!("SCD codec {other}"))),
    }
}

pub fn to_wav(scd: &[u8]) -> Result<Vec<u8>> {
    let i = parse(scd)?;
    if i.codec != CODEC_MS_ADPCM {
        return Err(FormatError::Unsupported("not an MS-ADPCM SCD".into()));
    }
    let num2 = i.stream_header_offset + 32;
    let fmt = get(scd, num2, 18)?;
    let cb_size = LE::read_u16(&fmt[16..18]) as usize;
    let extra = get(scd, num2 + 18, cb_size)?;
    let data = get(scd, i.ogg_data_offset, i.stream_size as usize)?;

    let mut w = Vec::with_capacity(50 + cb_size + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&((50 + cb_size + data.len()) as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&((18 + cb_size) as u32).to_le_bytes());
    w.extend_from_slice(&fmt[..18]);
    w.extend_from_slice(extra);
    w.extend_from_slice(b"fact");
    w.extend_from_slice(&4u32.to_le_bytes());
    w.extend_from_slice(&i.stream_size.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&i.stream_size.to_le_bytes());
    w.extend_from_slice(data);
    Ok(w)
}

/// Requires the WAV's fmt-extra block to be the same size as the SCD's.
fn replace_wav(scd: &[u8], wav: &[u8]) -> Result<Vec<u8>> {
    let i = parse(scd)?;
    let (fmt, data) = parse_wav(wav)?;
    let num2 = i.stream_header_offset + 32;
    let orig_cb = LE::read_u16(get(scd, num2 + 16, 2)?) as usize;
    let new_cb = if fmt.len() >= 18 {
        LE::read_u16(&fmt[16..18]) as usize
    } else {
        0
    };
    if fmt.len() != 18 + orig_cb || new_cb != orig_cb {
        return Err(FormatError::Unsupported(
            "WAV fmt size differs from the SCD (MS-ADPCM coefficient mismatch)".into(),
        ));
    }
    // The fmt write must land inside the copied header prefix, not the audio data.
    if scd.len() < i.ogg_data_offset || num2 + fmt.len() > i.ogg_data_offset {
        return Err(FormatError::Malformed {
            format: "SCD",
            detail: "stream header does not fit before the data offset".into(),
        });
    }
    let mut out = Vec::with_capacity(i.ogg_data_offset + data.len());
    out.extend_from_slice(&scd[..i.ogg_data_offset]);
    out.extend_from_slice(data);
    if let Some(tail) = scd.get(i.ogg_data_offset + i.stream_size as usize..) {
        out.extend_from_slice(tail);
    }
    out[num2..num2 + fmt.len()].copy_from_slice(fmt);
    out[i.stream_header_offset..i.stream_header_offset + 4]
        .copy_from_slice(&(data.len() as u32).to_le_bytes());
    let total = out.len() as u32;
    out[16..20].copy_from_slice(&total.to_le_bytes());
    Ok(out)
}

fn parse_wav(wav: &[u8]) -> Result<(&[u8], &[u8])> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(FormatError::BadMagic {
            expected: "RIFF/WAVE".into(),
            found: "?".into(),
        });
    }
    let (mut fmt, mut data) = (None, None);
    let mut p = 12;
    while p + 8 <= wav.len() {
        let id = &wav[p..p + 4];
        let size = LE::read_u32(&wav[p + 4..p + 8]) as usize;
        let body = wav
            .get(p + 8..p + 8 + size)
            .ok_or_else(|| wav_err("chunk past end"))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        p += 8 + size + (size & 1);
    }
    Ok((
        fmt.ok_or_else(|| wav_err("no fmt chunk"))?,
        data.ok_or_else(|| wav_err("no data chunk"))?,
    ))
}

fn wav_err(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "WAV",
        detail: detail.to_string(),
    }
}

fn get(buf: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len)
        .and_then(|end| buf.get(at..end))
        .ok_or_else(|| FormatError::Malformed {
            format: "SCD",
            detail: "read past end".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_scd() -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[SCD_HEADER_STREAM_PTR..SCD_HEADER_STREAM_PTR + 4].copy_from_slice(&64i32.to_le_bytes());
        b[64..68].copy_from_slice(&80i32.to_le_bytes());
        let n = 80;
        b[n..n + 4].copy_from_slice(&1234u32.to_le_bytes());
        b[n + 4..n + 8].copy_from_slice(&2u32.to_le_bytes());
        b[n + 8..n + 12].copy_from_slice(&44100u32.to_le_bytes());
        b[n + 12..n + 16].copy_from_slice(&CODEC_VORBIS.to_le_bytes());
        b[n + 24..n + 28].copy_from_slice(&0u32.to_le_bytes());
        b[n + 32..n + 34].copy_from_slice(&0i16.to_le_bytes());
        b
    }

    #[test]
    fn info_parses_synthetic_header() {
        let info = info(&synthetic_scd()).unwrap();
        assert_eq!(info.codec, CODEC_VORBIS);
        assert_eq!(info.channels, 2);
        assert_eq!(info.samplerate, 44100);
        assert_eq!(info.stream_size, 1234);
        assert_eq!(info.stream_header_offset, 80);
    }

    #[test]
    fn parse_skips_dummy_subsong() {
        let mut b = vec![0u8; 200];
        b[SCD_HEADER_STREAM_PTR..SCD_HEADER_STREAM_PTR + 4].copy_from_slice(&64i32.to_le_bytes());
        b[SCD_HEADER_STREAM_COUNT..SCD_HEADER_STREAM_COUNT + 2]
            .copy_from_slice(&2u16.to_le_bytes());
        b[64..68].copy_from_slice(&100i32.to_le_bytes());
        b[68..72].copy_from_slice(&130i32.to_le_bytes());
        b[112..116].copy_from_slice(&(-1i32).to_le_bytes());
        let n = 130;
        b[n..n + 4].copy_from_slice(&1234u32.to_le_bytes());
        b[n + 4..n + 8].copy_from_slice(&2u32.to_le_bytes());
        b[n + 8..n + 12].copy_from_slice(&44100u32.to_le_bytes());
        b[n + 12..n + 16].copy_from_slice(&CODEC_VORBIS.to_le_bytes());

        let info = info(&b).unwrap();
        assert_eq!(info.codec, CODEC_VORBIS);
        assert_eq!(info.stream_header_offset, 130);
        assert_eq!(info.samplerate, 44100);
    }

    #[test]
    fn extract_real_if_present() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let fl = crate::Filelist::read(format!("{dir}/sys/filelistu.win32.bin"), crate::Game::XIII)
            .unwrap();
        let mut img = std::fs::File::open(format!("{dir}/sys/white_imgu.win32.bin")).unwrap();
        let scds: Vec<_> = fl
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".scd"))
            .collect();
        let mut codecs = std::collections::BTreeMap::<i32, usize>::new();
        let mut vorbis_example = None;
        for e in scds.iter().step_by(scds.len() / 200 + 1) {
            let scd = fl.extract(&mut img, e).unwrap();
            let i = info(&scd).unwrap();
            *codecs.entry(i.codec).or_default() += 1;
            if i.codec == CODEC_VORBIS && vorbis_example.is_none() {
                vorbis_example = Some((e.path.clone(), i.encode_type));
            }
        }
        eprintln!(
            "codec distribution over {} sampled SCDs: {codecs:?}",
            scds.len().min(200)
        );
        eprintln!("vorbis example: {vorbis_example:?}");

        if let Some(music) = scds.iter().find(|e| e.path.contains("music")) {
            let scd = fl.extract(&mut img, music).unwrap();
            let i = info(&scd).unwrap();
            if i.codec == CODEC_VORBIS {
                let ogg = to_ogg(&scd).unwrap();
                assert_eq!(&ogg[0..4], b"OggS", "music SCD did not yield an Ogg");
                if i.encode_type == 0 {
                    let scd2 = replace_ogg(&scd, &ogg).unwrap();
                    assert_eq!(
                        to_ogg(&scd2).unwrap(),
                        ogg,
                        "music replace round-trip mismatch"
                    );
                    eprintln!("music SCD↔OGG round-trip OK: {}", music.path);
                }
            }
        }

        if let Some(sfx) = scds.iter().find(|e| e.path.contains("system_kettei")) {
            let scd = fl.extract(&mut img, sfx).unwrap();
            if info(&scd).unwrap().codec == CODEC_MS_ADPCM {
                let (ext, wav) = extract(&scd).unwrap();
                assert_eq!(ext, "wav");
                assert_eq!(&wav[0..4], b"RIFF", "SFX SCD did not yield a WAV");
                let scd2 = replace(&scd, &wav).unwrap();
                assert_eq!(
                    extract(&scd2).unwrap().1,
                    wav,
                    "SFX replace round-trip mismatch"
                );
                eprintln!("SFX SCD↔WAV round-trip OK: {}", sfx.path);
            }
        }
    }
}
