//! What is actually on disk: the MP4's top-level boxes, read directly.
//!
//! This is the half of "is the killed file playable" that needs no tool at
//! all. A fragmented MP4 is `ftyp`, then a `moov` that declares the tracks and
//! carries an `mvex` (the marker that fragments follow), then any number of
//! `moof` + `mdat` pairs, each of which stands on its own. A kill leaves some
//! prefix of that: the question is whether the prefix has a `moov` up front
//! and at least one complete fragment, and how much of the tail is a
//! half-written box that a player has to ignore.
//!
//! Only box headers are read, never the media, so a ten-minute file costs a
//! few thousand small reads.

use std::io::{Read, Seek, SeekFrom};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxEntry {
    pub kind: String,
    pub offset: u64,
    /// The size the header declares. For a truncated box this runs past the
    /// end of the file.
    pub size: u64,
    /// 8, or 16 when the header carries a 64-bit size.
    pub header: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub file_len: u64,
    pub ftyp: bool,
    pub moov_offset: Option<u64>,
    /// `moov` contains `mvex`: the file declares itself fragmented.
    pub mvex: bool,
    /// Tracks declared in `moov` (`trak` children).
    pub tracks: u32,
    pub moof: u32,
    pub mdat: u32,
    /// `moof` boxes immediately followed by a complete `mdat`: the fragments
    /// a player can actually use.
    pub complete_fragments: u32,
    pub first_moof_offset: Option<u64>,
    pub first_mdat_offset: Option<u64>,
    /// The fragment index a clean finalize writes last. Its absence in a
    /// killed file is expected and harmless.
    pub mfra: bool,
    /// A box whose declared size runs past the end of the file: the write
    /// the kill interrupted. `(kind, declared size, bytes present)`.
    pub truncated: Option<(String, u64, u64)>,
    /// Bytes at the end too short to hold a box header at all.
    pub trailing_garbage: u64,
    /// Every top-level box type, in order, with runs collapsed
    /// (`moof mdat x120`), for the report.
    pub layout: String,
}

impl Summary {
    /// The structural half of "playable": a header up front that says
    /// fragments follow, and at least one whole fragment after it.
    pub fn structurally_playable(&self) -> bool {
        match (self.moov_offset, self.first_moof_offset) {
            (Some(moov), Some(moof)) => {
                self.ftyp && self.mvex && moov < moof && self.complete_fragments > 0
            }
            _ => false,
        }
    }
}

fn read_header<R: Read + Seek>(
    r: &mut R,
    offset: u64,
    end: u64,
) -> std::io::Result<Option<BoxEntry>> {
    if end.saturating_sub(offset) < 8 {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(offset))?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let kind: String = head[4..8]
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect();
    let (size, header) = match size32 {
        // "To the end of the file", which only the last box may say.
        0 => (end - offset, 8),
        1 => {
            if end - offset < 16 {
                return Ok(None);
            }
            let mut large = [0u8; 8];
            r.read_exact(&mut large)?;
            (u64::from_be_bytes(large), 16)
        }
        n => (u64::from(n), 8),
    };
    Ok(Some(BoxEntry {
        kind,
        offset,
        size,
        header,
    }))
}

/// The direct children of the box at `entry`, as far as they are present.
fn children<R: Read + Seek>(
    r: &mut R,
    entry: &BoxEntry,
    file_len: u64,
) -> std::io::Result<Vec<BoxEntry>> {
    let end = (entry.offset + entry.size).min(file_len);
    let mut offset = entry.offset + entry.header;
    let mut out = Vec::new();
    while let Some(child) = read_header(r, offset, end)? {
        if child.size < 8 {
            break;
        }
        offset = child.offset + child.size;
        out.push(child);
    }
    Ok(out)
}

pub fn summarize<R: Read + Seek>(r: &mut R) -> std::io::Result<Summary> {
    let file_len = r.seek(SeekFrom::End(0))?;
    let mut summary = Summary {
        file_len,
        ..Summary::default()
    };
    let mut boxes: Vec<BoxEntry> = Vec::new();
    // Whether the last entry in `boxes` is the one the kill cut short.
    let mut last_is_truncated = false;
    let mut offset = 0u64;
    loop {
        let Some(entry) = read_header(r, offset, file_len)? else {
            summary.trailing_garbage = file_len - offset;
            break;
        };
        if entry.size < 8 {
            // A zero-length or nonsense header: nothing after it can be found.
            summary.truncated = Some((entry.kind.clone(), entry.size, file_len - offset));
            break;
        }
        let end = entry.offset + entry.size;
        if end > file_len {
            summary.truncated = Some((entry.kind.clone(), entry.size, file_len - entry.offset));
            boxes.push(entry);
            last_is_truncated = true;
            break;
        }
        offset = end;
        boxes.push(entry);
    }

    let count = boxes.len();
    let complete = |i: usize| -> bool { !(last_is_truncated && i + 1 == count) };

    for (i, entry) in boxes.iter().enumerate() {
        match entry.kind.as_str() {
            "ftyp" => summary.ftyp = true,
            "moov" => {
                summary.moov_offset.get_or_insert(entry.offset);
                if complete(i) {
                    for child in children(r, entry, file_len)? {
                        match child.kind.as_str() {
                            "mvex" => summary.mvex = true,
                            "trak" => summary.tracks += 1,
                            _ => {}
                        }
                    }
                }
            }
            "moof" => {
                summary.moof += 1;
                summary.first_moof_offset.get_or_insert(entry.offset);
                if boxes.get(i + 1).is_some_and(|next| next.kind == "mdat") && complete(i + 1) {
                    summary.complete_fragments += 1;
                }
            }
            "mdat" => {
                summary.mdat += 1;
                summary.first_mdat_offset.get_or_insert(entry.offset);
            }
            "mfra" => summary.mfra = true,
            _ => {}
        }
    }

    summary.layout = collapse(&boxes);
    Ok(summary)
}

/// `ftyp moov moof mdat moof mdat ... mfra` as `ftyp moov (moof mdat) x2 mfra`.
fn collapse(boxes: &[BoxEntry]) -> String {
    let kinds: Vec<&str> = boxes.iter().map(|b| b.kind.as_str()).collect();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] == "moof" && kinds.get(i + 1) == Some(&"mdat") {
            let mut n = 0;
            while kinds.get(i) == Some(&"moof") && kinds.get(i + 1) == Some(&"mdat") {
                n += 1;
                i += 2;
            }
            parts.push(format!("(moof mdat) x{n}"));
        } else {
            parts.push(kinds[i].to_string());
            i += 1;
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn bx(kind: &str, body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind.as_bytes());
        out.extend_from_slice(body);
        out
    }

    fn fragmented(fragments: usize) -> Vec<u8> {
        let mut file = bx("ftyp", b"iso6\0\0\0\0");
        let mut moov = bx("mvhd", &[0; 100]);
        moov.extend(bx("trak", &[0; 20]));
        moov.extend(bx("trak", &[0; 20]));
        moov.extend(bx("mvex", &bx("trex", &[0; 24])));
        file.extend(bx("moov", &moov));
        for _ in 0..fragments {
            file.extend(bx("moof", &[0; 64]));
            file.extend(bx("mdat", &[0xAB; 1000]));
        }
        file
    }

    #[test]
    fn a_finished_fragmented_file() {
        let mut file = fragmented(3);
        file.extend(bx("mfra", &[0; 16]));
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert!(s.ftyp && s.mvex && s.mfra);
        assert_eq!(s.tracks, 2);
        assert_eq!((s.moof, s.mdat, s.complete_fragments), (3, 3, 3));
        assert_eq!(s.truncated, None);
        assert_eq!(s.layout, "ftyp moov (moof mdat) x3 mfra");
        assert!(s.structurally_playable());
    }

    #[test]
    fn a_file_killed_mid_mdat_keeps_its_whole_fragments() {
        let mut file = fragmented(3);
        let cut = file.len() - 400;
        file.truncate(cut);
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert_eq!(s.moof, 3);
        assert_eq!(s.complete_fragments, 2);
        let (kind, declared, present) = s.truncated.clone().unwrap();
        assert_eq!(kind, "mdat");
        assert_eq!(declared, 1008);
        assert_eq!(present, 608);
        assert!(!s.mfra);
        assert!(s.structurally_playable());
    }

    #[test]
    fn a_file_killed_mid_header_is_counted_as_garbage() {
        let mut file = fragmented(1);
        file.extend_from_slice(&[0, 0, 1]);
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert_eq!(s.trailing_garbage, 3);
        assert_eq!(s.complete_fragments, 1);
    }

    #[test]
    fn an_ordinary_mp4_is_not_fragmented() {
        let mut file = bx("ftyp", b"isom\0\0\0\0");
        file.extend(bx("mdat", &[0; 500]));
        file.extend(bx("moov", &bx("trak", &[0; 20])));
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert!(!s.mvex);
        assert!(!s.structurally_playable());
    }

    #[test]
    fn a_header_with_no_fragments_is_not_playable() {
        let s = summarize(&mut Cursor::new(fragmented(0))).unwrap();
        assert!(s.mvex);
        assert!(!s.structurally_playable());
    }

    #[test]
    fn a_large_size_header_is_read() {
        let mut file = bx("ftyp", b"iso6\0\0\0\0");
        let mut large = 1u32.to_be_bytes().to_vec();
        large.extend_from_slice(b"mdat");
        large.extend_from_slice(&(16u64 + 32).to_be_bytes());
        large.extend_from_slice(&[0; 32]);
        file.extend(large);
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert_eq!(s.mdat, 1);
        assert_eq!(s.truncated, None);
    }
}
