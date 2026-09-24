//! What is actually on disk: the MP4's top-level boxes, read directly.
//!
//! Ported from the `p0c-video` spike (`spikes/p0c-video/src/mp4.rs`), where
//! it answered "is the killed file playable" on the box. Here it answers the
//! same question for `db::reconcile::recover_unfinished`, which has to decide
//! what to do with a recording a dead daemon left behind before it spends an
//! ffmpeg run on it.
//!
//! This is the half of that question that needs no tool at all. A fragmented
//! MP4 is `ftyp`, then a `moov` that declares the tracks and carries an `mvex`
//! (the marker that fragments follow), then any number of `moof` + `mdat`
//! pairs, each of which stands on its own. A kill leaves some prefix of that:
//! the question is whether the prefix has a `moov` up front and at least one
//! complete fragment, and how much of the tail is a half-written box that a
//! player has to ignore.
//!
//! Only box headers are read, never the media, so a ten-minute file costs a
//! few thousand small reads. The one exception is four bytes per track, the
//! `hdlr` handler type, which is how the audio tracks are counted.

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

/// What a file's top-level boxes say about it. Every field is a fact read
/// from the file; nothing here is a judgement except `structurally_playable`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub file_len: u64,
    pub ftyp: bool,
    pub moov_offset: Option<u64>,
    /// The `moov` is all there. A kill during the header write leaves one
    /// whose declared size runs past the end of the file, and nothing in it
    /// can be trusted.
    pub moov_complete: bool,
    /// `moov` contains `mvex`: the file declares itself fragmented.
    pub mvex: bool,
    /// Tracks declared in `moov` (`trak` children).
    pub tracks: u32,
    /// Of those, the ones whose handler is `soun`. What a remux needs to
    /// know to mark track 0 as the default, and what an unfinished row
    /// cannot say, because its `audio_tracks_json` is written at finalize.
    pub audio_tracks: u32,
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
    /// the kill interrupted.
    pub truncated: Option<Truncated>,
    /// Bytes at the end too short to hold a box header at all.
    pub trailing_garbage: u64,
    /// Every top-level box type, in order, with runs collapsed
    /// (`moof mdat x120`), for the log.
    pub layout: String,
}

/// The box a kill cut short.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Truncated {
    pub kind: String,
    /// Where the box starts. Everything before it is whole.
    pub offset: u64,
    /// The size its header declares.
    pub declared: u64,
    /// How much of it made it to disk, header included.
    pub present: u64,
}

impl Summary {
    /// The structural half of "playable": a header up front that says
    /// fragments follow, and at least one whole fragment after it.
    pub fn structurally_playable(&self) -> bool {
        match (self.moov_offset, self.first_moof_offset) {
            (Some(moov), Some(moof)) => {
                self.ftyp
                    && self.moov_complete
                    && self.mvex
                    && moov < moof
                    && self.complete_fragments > 0
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

/// Whether a `trak` is an audio track: `trak/mdia/hdlr`'s handler type is
/// `soun`. `hdlr` is a full box, so the type follows four bytes of version
/// and flags and four of `pre_defined`.
fn is_audio_track<R: Read + Seek>(
    r: &mut R,
    trak: &BoxEntry,
    file_len: u64,
) -> std::io::Result<bool> {
    let Some(mdia) = children(r, trak, file_len)?.into_iter().find(|b| b.kind == "mdia") else {
        return Ok(false);
    };
    let Some(hdlr) = children(r, &mdia, file_len)?.into_iter().find(|b| b.kind == "hdlr") else {
        return Ok(false);
    };
    let at = hdlr.offset + hdlr.header + 8;
    if at + 4 > (hdlr.offset + hdlr.size).min(file_len) {
        return Ok(false);
    }
    r.seek(SeekFrom::Start(at))?;
    let mut handler = [0u8; 4];
    r.read_exact(&mut handler)?;
    Ok(&handler == b"soun")
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
            summary.truncated = Some(Truncated {
                kind: entry.kind.clone(),
                offset,
                declared: entry.size,
                present: file_len - offset,
            });
            break;
        }
        let end = entry.offset + entry.size;
        if end > file_len {
            summary.truncated = Some(Truncated {
                kind: entry.kind.clone(),
                offset: entry.offset,
                declared: entry.size,
                present: file_len - entry.offset,
            });
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
                    summary.moov_complete = true;
                    for child in children(r, entry, file_len)? {
                        match child.kind.as_str() {
                            "mvex" => summary.mvex = true,
                            "trak" => {
                                summary.tracks += 1;
                                if is_audio_track(r, &child, file_len)? {
                                    summary.audio_tracks += 1;
                                }
                            }
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
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    pub(crate) fn bx(kind: &str, body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind.as_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A `trak` carrying just enough of `mdia/hdlr` to say what it is.
    pub(crate) fn trak(handler: &[u8; 4]) -> Vec<u8> {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0; 12]);
        bx("trak", &bx("mdia", &bx("hdlr", &hdlr)))
    }

    /// One video track and `audio` audio tracks, then `fragments` fragments.
    pub(crate) fn fragmented_with(audio: usize, fragments: usize) -> Vec<u8> {
        let mut file = bx("ftyp", b"iso6\0\0\0\0");
        let mut moov = bx("mvhd", &[0; 100]);
        moov.extend(trak(b"vide"));
        for _ in 0..audio {
            moov.extend(trak(b"soun"));
        }
        moov.extend(bx("mvex", &bx("trex", &[0; 24])));
        file.extend(bx("moov", &moov));
        for _ in 0..fragments {
            file.extend(bx("moof", &[0; 64]));
            file.extend(bx("mdat", &[0xAB; 1000]));
        }
        file
    }

    fn fragmented(fragments: usize) -> Vec<u8> {
        fragmented_with(1, fragments)
    }

    #[test]
    fn a_finished_fragmented_file() {
        let mut file = fragmented(3);
        file.extend(bx("mfra", &[0; 16]));
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert!(s.ftyp && s.moov_complete && s.mvex && s.mfra);
        assert_eq!((s.tracks, s.audio_tracks), (2, 1));
        assert_eq!((s.moof, s.mdat, s.complete_fragments), (3, 3, 3));
        assert_eq!(s.truncated, None);
        assert_eq!(s.layout, "ftyp moov (moof mdat) x3 mfra");
        assert!(s.structurally_playable());
    }

    #[test]
    fn audio_tracks_are_counted_by_handler() {
        let s = summarize(&mut Cursor::new(fragmented_with(4, 1))).unwrap();
        assert_eq!((s.tracks, s.audio_tracks), (5, 4));
        let s = summarize(&mut Cursor::new(fragmented_with(0, 1))).unwrap();
        assert_eq!((s.tracks, s.audio_tracks), (1, 0));
    }

    #[test]
    fn a_file_killed_mid_mdat_keeps_its_whole_fragments() {
        let mut file = fragmented(3);
        let cut = file.len() - 400;
        file.truncate(cut);
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert_eq!(s.moof, 3);
        assert_eq!(s.complete_fragments, 2);
        let t = s.truncated.clone().unwrap();
        assert_eq!(t.kind, "mdat");
        assert_eq!(t.declared, 1008);
        assert_eq!(t.present, 608);
        assert_eq!(t.offset, cut as u64 - 608);
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
    fn a_file_killed_mid_moov_has_no_usable_header() {
        let mut file = fragmented(0);
        let cut = file.len() - 10;
        file.truncate(cut);
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert!(s.moov_offset.is_some());
        assert!(!s.moov_complete);
        assert!(!s.mvex);
        assert_eq!(s.truncated.unwrap().kind, "moov");
    }

    #[test]
    fn an_ordinary_mp4_is_not_fragmented() {
        let mut file = bx("ftyp", b"isom\0\0\0\0");
        file.extend(bx("mdat", &[0; 500]));
        file.extend(bx("moov", &bx("trak", &[0; 20])));
        let s = summarize(&mut Cursor::new(file)).unwrap();
        assert!(!s.mvex);
        assert!(s.moov_complete);
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

    #[test]
    fn a_file_that_is_not_an_mp4_is_nothing_in_particular() {
        let s = summarize(&mut Cursor::new(b"partial but playable".to_vec())).unwrap();
        assert!(!s.ftyp && s.moov_offset.is_none());
        assert!(!s.structurally_playable());
    }
}
