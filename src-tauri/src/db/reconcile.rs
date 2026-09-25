//! Folder-scan reconciliation: a DB row whose file is gone gets removed;
//! a video file with no DB row gets imported as an "unknown recording".
//! DEVELOPMENT.md §4 — "the library must survive users touching the
//! folder." Run once at app startup and available on demand (dev panel /
//! future UI rescan button).
//!
//! `recover_unfinished` is the other half, and runs **at daemon startup
//! only**. A recording that was in flight when a daemon died leaves a row
//! with no `finished_at`, which `list_recordings` hides and `reconcile`'s
//! own orphan sweep therefore cannot see; its file is skipped by the import
//! pass too, because `find_by_path` finds the row. Without a pass that
//! finishes those rows from the file, a killed recording would be
//! *invisible* rather than merely stripped of its metadata, which is worse
//! than the bug #150 set out to fix.
//!
//! Recovery also remuxes what it finds (#233). A killed recording is a
//! fragmented MP4 that never reached the faststart remux a clean stop runs,
//! so without this it came back into the library playable but not
//! scrubbable. What to do with each file is `recovery_action`'s decision,
//! made from the file's own boxes (`mp4::read`) and nothing else.
//!
//! **A file the own backend wrote is repaired in Rust first** (#239):
//! `mp4::write::repair` cuts it to its last whole fragment and appends the
//! `mfra` the killed writer never did, which needs no ffmpeg. It refuses any
//! file it did not write, so a libobs recording goes straight to the remux,
//! as it always has. A repaired file is then remuxed like any other, because
//! a clean stop remuxes too, until the review player is shown to seek an
//! `mfra` file (DEVELOPMENT.md §2.5).

use super::{Db, DbError, NewRecording};
use crate::mp4::Summary;
use crate::{info, warn};
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, ts_rs::TS)]
pub struct ReconcileReport {
    pub orphans_removed: usize,
    pub imported: usize,
}

/// What `recover_unfinished` did. Both numbers are normally zero: an
/// unfinished row at startup means the previous daemon did not get to
/// finalize.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryReport {
    /// Rows finished from their file, keeping the markers written during
    /// the game.
    pub recovered: usize,
    /// Rows whose file is gone, deleted. A recording that never wrote a
    /// frame, or whose file the user removed before the daemon came back.
    pub abandoned_removed: usize,
}

/// Finishes the rows a dead daemon left open, from what their files can be
/// made to say.
///
/// **Startup only, and that is a correctness requirement, not a
/// preference.** An in-progress recording is indistinguishable here from an
/// abandoned one: both are a row with no `finished_at` and a file on disk.
/// Running this while the supervisor is recording would finish the row it is
/// still writing, putting a half-written file in the library and letting
/// retention delete it. The daemon calls this before it can start recording,
/// which is what makes the question safe to not ask.
///
/// Only the columns a file can answer for are written. The champion, the
/// KDA and the outcome died with the daemon that was polling for them, and
/// a default is a confident wrong answer where NULL is a true one. The
/// markers stay untouched: they are what this whole path exists to keep.
pub fn recover_unfinished(db: &Db, ffmpeg: Option<&Path>) -> Result<RecoveryReport, DbError> {
    let mut report = RecoveryReport::default();

    for row in db.unfinished_recordings()? {
        let path = Path::new(&row.path);
        let Ok(metadata) = path.metadata() else {
            // No file, so nothing to finish and nothing to show. This is
            // also the only sweep that can reach these rows, since
            // `reconcile`'s runs off `list_recordings`, which hides them.
            db.delete_recording(row.id)?;
            report.abandoned_removed += 1;
            continue;
        };
        // The file's mtime, not now, and read before the remux rewrites the
        // file: the recording ended when the daemon died, which may have
        // been days ago, and `finished_at` ordering a recovered recording
        // above everything since would be a lie the library sorts on.
        let finished_at = file_modified_millis(&metadata);

        // A remux the dead daemon had running leaves its half-written output
        // here. Nothing will ever finish it, and the remux below would only
        // overwrite it.
        remove_stale_remux_tmp(path);

        // Before the duration probe, so the probe reads the file the library
        // will play. Never fatal: the worst outcome is the file as the kill
        // left it, which is what recovery finished every row with before.
        repair(path, ffmpeg, &metadata);

        // The session's clock is gone, so the file is the only source of a
        // duration. `None` stays NULL, exactly as it does for an import that
        // could not be probed.
        let duration_s = ffmpeg.and_then(|ffmpeg| crate::probe::duration_s(ffmpeg, path));
        // Re-read: the remux and the tail cut both change it.
        let size_bytes = path.metadata().map_or(metadata.len(), |m| m.len());
        db.recover_recording(row.id, duration_s, size_bytes as i64, finished_at)?;
        report.recovered += 1;
    }

    Ok(report)
}

fn remove_stale_remux_tmp(path: &Path) {
    let tmp = crate::recorder::remux::tmp_path(path);
    match std::fs::remove_file(&tmp) {
        Ok(()) => info!("db", "removed a stale remux temp file {}", tmp.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!("db", "could not remove stale {}: {e}", tmp.display()),
    }
}

/// What recovery does with a file a dead daemon left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// A fragmented file with at least one whole fragment: remux it to
    /// faststart, as a clean stop would have.
    Remux {
        /// From the file's `moov`, because an unfinished row's
        /// `audio_tracks_json` is NULL: the layout is written at finalize.
        audio_tracks: usize,
        /// Cut the file to this length first. Set when the box the kill
        /// interrupted is anything but an `mdat`: a half-written `moof` stops
        /// ffmpeg opening the file at all, and carries no media, since its
        /// `mdat` never started. A half-written `mdat` is kept, because
        /// ffmpeg salvages the samples in it.
        truncate_to: Option<u64>,
    },
    /// Already a complete, unfragmented MP4, so there is no index to move:
    /// the faststart remux ran, and the daemon died after it.
    Leave,
    /// Nothing a remux can fix: no header, a header with no whole fragment
    /// after it, or not an MP4 at all. The row is still finished, for its
    /// markers.
    Unplayable,
}

/// The pure half of the repair: what the file's boxes say should happen.
pub fn recovery_action(summary: &Summary) -> RecoveryAction {
    if summary.structurally_playable() {
        let truncate_to = summary
            .truncated
            .as_ref()
            .filter(|cut| cut.kind != "mdat")
            .map(|cut| cut.offset);
        return RecoveryAction::Remux {
            audio_tracks: summary.audio_tracks as usize,
            truncate_to,
        };
    }
    if summary.ftyp && summary.moov_complete && !summary.mvex {
        return RecoveryAction::Leave;
    }
    RecoveryAction::Unplayable
}

/// The I/O half: repairs a file our own writer made, reads the boxes, acts
/// on `recovery_action`, and logs what it did. Every failure is logged and
/// swallowed, leaving the file as it is.
fn repair(path: &Path, ffmpeg: Option<&Path>, metadata: &std::fs::Metadata) {
    // Ours (the own backend's), or not: `repair` answers by refusing a file
    // it did not write, which is the libobs case and needs no log line.
    if let Ok(r) = crate::mp4::write::repair(path)
        && !r.already_complete
    {
        info!(
            "db",
            "repaired recovered {}: {} whole fragment(s) kept, {} torn byte(s) cut, mfra written",
            path.display(),
            r.fragments,
            r.removed_bytes
        );
        restore_mtime(path, metadata);
    }
    let summary = match std::fs::File::open(path).and_then(|mut f| crate::mp4::summarize(&mut f)) {
        Ok(summary) => summary,
        Err(e) => {
            warn!("db", "recovery could not read {}: {e}", path.display());
            return;
        }
    };
    let (audio_tracks, truncate_to) = match recovery_action(&summary) {
        RecoveryAction::Remux { audio_tracks, truncate_to } => (audio_tracks, truncate_to),
        RecoveryAction::Leave => return,
        RecoveryAction::Unplayable => {
            warn!(
                "db",
                "recovered {} cannot be repaired (boxes: {}); keeping it as it is",
                path.display(),
                summary.layout
            );
            return;
        }
    };

    // Whether or not there is an ffmpeg: the half box is what stops a player
    // opening the file, so cutting it helps even without the remux.
    if let Some(len) = truncate_to {
        let cut = std::fs::OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(len));
        match cut {
            Ok(()) => info!(
                "db",
                "dropped a half-written tail from {} ({} bytes)",
                path.display(),
                summary.file_len - len
            ),
            Err(e) => warn!("db", "could not drop the tail from {}: {e}", path.display()),
        }
    }

    let Some(ffmpeg) = ffmpeg else {
        warn!(
            "db",
            "no ffmpeg, so recovered {} stays fragmented and will not scrub",
            path.display()
        );
        return;
    };
    // Inline at startup, so the time is worth knowing: a long recording is
    // a gigabyte or more of copying before the daemon is up.
    let started = Instant::now();
    match crate::recorder::remux::remux_faststart(ffmpeg, path, audio_tracks) {
        Ok(()) => {
            info!(
                "db",
                "remuxed recovered {} ({}, {} audio track(s)) in {} ms",
                path.display(),
                summary.layout,
                audio_tracks,
                started.elapsed().as_millis()
            );
            restore_mtime(path, metadata);
        }
        Err(e) => warn!(
            "db",
            "faststart remux of recovered {} failed after {} ms, keeping it fragmented: {e}",
            path.display(),
            started.elapsed().as_millis()
        ),
    }
}

/// The repair and the remux both rewrite the file, so its mtime is now. Put
/// back the one the kill left, so that a recovery interrupted before the row
/// is written still dates the recording correctly next time.
fn restore_mtime(path: &Path, metadata: &std::fs::Metadata) {
    if let Ok(modified) = metadata.modified() {
        let restored =
            std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(modified));
        if let Err(e) = restored {
            warn!("db", "could not restore the mtime of {}: {e}", path.display());
        }
    }
}

/// `ffmpeg` is the bundled (or locally installed) binary, if this build has
/// one. It is used to read a duration out of each file being imported, and
/// `None` simply leaves that column NULL — see `probe`.
pub fn reconcile(
    db: &Db,
    recordings_dir: &Path,
    ffmpeg: Option<&Path>,
) -> Result<ReconcileReport, DbError> {
    let mut report = ReconcileReport::default();

    for row in db.list_recordings()? {
        if !Path::new(&row.path).exists() {
            db.delete_recording(row.id)?;
            report.orphans_removed += 1;
        }
    }

    if recordings_dir.exists() {
        for entry in std::fs::read_dir(recordings_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || !is_video_file(&path) {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();
            if db.find_by_path(&path_str)?.is_some() {
                continue;
            }

            let metadata = entry.metadata()?;
            // Once per file, on the import path only: the `find_by_path`
            // skip above means a rescan of a settled folder spawns nothing.
            // A first run against a large existing folder is the case that
            // costs — one ffmpeg per file, and startup reconcile is inline
            // (`lib.rs`'s `setup`), so it is startup latency.
            //
            // A recording in flight no longer reaches here: its row is
            // written when capture starts, so `find_by_path` above finds it
            // and this loop skips the file (#150). What is left is a file the
            // user put in the folder, or one whose row was deleted.
            //
            // The race is narrower than it was but not gone. A recording that
            // began before this version shipped has no row, and a start-insert
            // that failed leaves none either; in both cases the growing file
            // is still importable here, and a finalize landing between the
            // lookup and the insert leaves the probed duration in place. That
            // costs a slightly short length on one recording, and
            // `finish_recording` deletes the imported row when it lands.
            let duration_s = ffmpeg.and_then(|ffmpeg| crate::probe::duration_s(ffmpeg, &path));
            let modified = file_modified_millis(&metadata);
            db.insert_recording(&NewRecording {
                path: path_str,
                started_at: modified,
                size_bytes: metadata.len() as i64,
                duration_s,
                // **Imported files are finished, and saying so is not
                // optional** (#150). A row with no `finished_at` is hidden
                // from the library, so leaving this `None` would import every
                // untracked recording straight into invisibility.
                //
                // The file's own mtime is the only end time available. It is
                // the same value `started_at` gets, which is honest about how
                // little is known: a file found on disk has no session behind
                // it to ask.
                finished_at: Some(modified),
                ..Default::default()
            })?;
            report.imported += 1;
        }
    }

    Ok(report)
}

/// Builds before #233 named the remux's temp file `<stem>.faststart.tmp.mp4`,
/// so one a crash left behind would pass the extension check below. The name
/// is ours and never a recording, so it is refused by name.
const LEGACY_REMUX_TMP_SUFFIX: &str = ".faststart.tmp.mp4";

fn is_video_file(path: &Path) -> bool {
    let legacy_tmp = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_lowercase().ends_with(LEGACY_REMUX_TMP_SUFFIX));
    if legacy_tmp {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn file_modified_millis(metadata: &std::fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-reconcile-test-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removes_db_row_whose_file_is_gone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("orphan");
        db.insert_recording(&NewRecording {
            path: dir.join("gone.mp4").to_string_lossy().to_string(),
            started_at: 1,
            finished_at: Some(1),
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.orphans_removed, 1);
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imports_untracked_video_file() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("import");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.orphans_removed, 0);
        assert_eq!(report.imported, 1);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].path.ends_with("untracked.mp4"));
        assert!(rows[0].champion.is_none(), "imported rows carry no match metadata");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_without_an_ffmpeg_leaves_duration_unknown() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("no-ffmpeg");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        assert_eq!(reconcile(&db, &dir, None).unwrap().imported, 1);
        assert_eq!(db.list_recordings().unwrap()[0].duration_s, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ffmpeg_that_cannot_be_run_still_imports_the_file() {
        // The failure-tolerance the issue asks for: reconcile must not
        // start failing over a cosmetic column. A path that isn't a binary
        // stands in for every way the probe can come back empty.
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("broken-ffmpeg");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        let report = reconcile(&db, &dir, Some(Path::new("/definitely/not/ffmpeg"))).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(db.list_recordings().unwrap()[0].duration_s, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ignores_non_video_files() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("ignore");
        std::fs::write(dir.join("notes.txt"), b"not a video").unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tracked_file_is_left_alone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("tracked");
        let file_path = dir.join("known.mp4");
        std::fs::write(&file_path, b"fake video bytes").unwrap();
        db.insert_recording(&NewRecording {
            path: file_path.to_string_lossy().to_string(),
            started_at: 1,
            champion: Some("Ahri".into()),
            finished_at: Some(1),
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report, ReconcileReport::default());

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].champion, Some("Ahri".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Recovering what a killed daemon left open (#150) -----------------

    /// The acceptance test for the second half of #150. A daemon that died
    /// mid-recording left a row with no `finished_at`, holding the markers
    /// written during the game. Startup has to finish it from the file, and
    /// **keep the markers**, which are the whole point of the row existing.
    #[test]
    fn recovery_finishes_an_abandoned_row_and_keeps_its_markers() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover");
        let file = dir.join("recording-1.mp4");
        std::fs::write(&file, b"partial but playable").unwrap();

        let id = db
            .begin_recording(&file.to_string_lossy(), 1_000)
            .unwrap();
        db.insert_markers(
            id,
            &[crate::db::NewMarker {
                game_time_s: 210.5,
                video_time_s: 210.5,
                kind: "kill".into(),
                payload_json: "{}".into(),
            }],
        )
        .unwrap();

        assert!(db.list_recordings().unwrap().is_empty(), "hidden while unfinished");

        let report = recover_unfinished(&db, None).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(report.abandoned_removed, 0);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1, "it is a library entry now");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].size_bytes, "partial but playable".len() as i64);
        assert_eq!(
            db.get_markers(id).unwrap().len(),
            1,
            "the markers are what survived the kill; recovery must not touch them"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A killed own-backend recording (#239): whole fragments, then a `moof`
    /// the kill cut short, and no `mfra`. Recovery repairs it in Rust with no
    /// ffmpeg at all: the torn tail goes, the `mfra` is written, every track
    /// is still declared, and the file keeps the mtime the kill left.
    #[test]
    fn recovery_repairs_a_killed_own_recording_without_ffmpeg() {
        use crate::mp4::write::{Track, Writer};
        // libx264's 320x240 Constrained Baseline SPS, and a PPS.
        const SPS: [u8; 23] = [
            0x67, 0x42, 0xc0, 0x15, 0xd9, 0x01, 0x41, 0xfb, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00,
            0x10, 0x00, 0x00, 0x07, 0x80, 0xf1, 0x62, 0xe4, 0x80,
        ];
        const PPS: [u8; 4] = [0x68, 0xce, 0x38, 0x80];

        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-own");
        let file = dir.join("recording-1.mp4");
        let tracks = vec![
            Track::h264(&SPS, &PPS).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
        ];
        let mut writer = Writer::create(&file, tracks).unwrap();
        for fragment in 0..2u64 {
            if fragment > 0 {
                writer.flush_fragment().unwrap();
            }
            for frame in 0..10u64 {
                let key = frame == 0;
                let au = [0, 0, 0, 1, if key { 0x65 } else { 0x41 }, 0x88, frame as u8, 1];
                writer.write_sample(0, (fragment * 10 + frame) * 1500, 1500, key, &au).unwrap();
                for track in 1..4 {
                    let pts = (fragment * 10 + frame) * 1024;
                    writer.write_sample(track, pts, 1024, true, &[0x21, track as u8]).unwrap();
                }
            }
        }
        writer.flush_fragment().unwrap();
        drop(writer);
        // The kill: half a `moof` header after the last whole fragment.
        let whole = std::fs::metadata(&file).unwrap().len();
        let mut torn = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        std::io::Write::write_all(&mut torn, &[0, 0, 1, 0, b'm', b'o', b'o', b'f', 0, 0]).unwrap();
        drop(torn);
        let killed_at = std::fs::metadata(&file).unwrap().modified().unwrap();
        let id = db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();

        let report = recover_unfinished(&db, None).unwrap();
        assert_eq!(report.recovered, 1);

        let summary = crate::mp4::summarize(&mut std::fs::File::open(&file).unwrap()).unwrap();
        assert!(summary.mfra && summary.structurally_playable(), "{summary:?}");
        assert_eq!(summary.truncated, None, "{summary:?}");
        assert_eq!((summary.tracks, summary.audio_tracks, summary.complete_fragments), (4, 3, 2));
        assert!(summary.file_len > whole, "the mfra is appended after the whole fragments");
        let rows = db.list_recordings().unwrap();
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].size_bytes, summary.file_len as i64);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), killed_at);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing to show and nothing to keep. This is also the only sweep that
    /// can reach such a row: `reconcile`'s orphan pass runs off
    /// `list_recordings`, which hides unfinished rows by design.
    #[test]
    fn recovery_deletes_an_abandoned_row_whose_file_is_gone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-no-file");
        let missing = dir.join("never-written.mp4");

        db.begin_recording(&missing.to_string_lossy(), 1_000).unwrap();

        let report = recover_unfinished(&db, None).unwrap();
        assert_eq!(report.abandoned_removed, 1);
        assert_eq!(report.recovered, 0);
        assert!(db.unfinished_recordings().unwrap().is_empty());
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A finished row is not this pass's business, and rewriting its
    /// duration from a probe would undo the session clock's better answer.
    #[test]
    fn recovery_leaves_finished_rows_alone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-finished");
        let file = dir.join("done.mp4");
        std::fs::write(&file, b"fake video bytes").unwrap();

        db.insert_recording(&NewRecording {
            path: file.to_string_lossy().to_string(),
            started_at: 1,
            duration_s: Some(1234.5),
            champion: Some("Ahri".into()),
            finished_at: Some(2),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(recover_unfinished(&db, None).unwrap(), RecoveryReport::default());
        let rows = db.list_recordings().unwrap();
        assert_eq!(rows[0].duration_s, Some(1234.5));
        assert_eq!(rows[0].champion.as_deref(), Some("Ahri"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other thing the start-insert buys. `reconcile` used to import an
    /// in-progress recording's growing file as an "unknown recording",
    /// because it could not tell one from a file the user had dropped in.
    /// Now the row exists, `find_by_path` finds it, and the scan skips it.
    #[test]
    fn an_in_progress_recording_is_not_imported_as_an_unknown_recording() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("in-progress");
        let file = dir.join("recording-2.mp4");
        std::fs::write(&file, b"still being written").unwrap();
        db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.imported, 0, "the file is ours and already tracked");
        assert_eq!(report.orphans_removed, 0);
        assert_eq!(db.unfinished_recordings().unwrap().len(), 1, "still in flight");
        assert!(db.list_recordings().unwrap().is_empty(), "and still not in the library");

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Repairing what recovery finds (#233) ------------------------------

    use crate::mp4::read::tests::{bx, fragmented_with, trak};
    use crate::recorder::remux::fixtures;

    fn action_for(bytes: Vec<u8>) -> RecoveryAction {
        recovery_action(&crate::mp4::summarize(&mut std::io::Cursor::new(bytes)).unwrap())
    }

    /// A clean libobs stop that died before its remux: the fragments are all
    /// there, the index is not up front.
    #[test]
    fn a_finished_fragmented_file_is_remuxed() {
        let mut file = fragmented_with(2, 3);
        file.extend(bx("mfra", &[0; 16]));
        assert_eq!(
            action_for(file),
            RecoveryAction::Remux { audio_tracks: 2, truncate_to: None }
        );
    }

    /// The case this exists for. The half `mdat` stays: ffmpeg keeps what
    /// samples it can from it.
    #[test]
    fn a_file_killed_mid_mdat_is_remuxed_as_it_is() {
        let mut file = fragmented_with(4, 3);
        file.truncate(file.len() - 400);
        assert_eq!(
            action_for(file),
            RecoveryAction::Remux { audio_tracks: 4, truncate_to: None }
        );
    }

    /// A half `moof` stops ffmpeg opening the file, and has no media behind
    /// it, so it is cut off first.
    #[test]
    fn a_file_killed_mid_moof_loses_the_half_box_first() {
        let mut file = fragmented_with(1, 2);
        let whole = file.len() as u64;
        file.extend(bx("moof", &[0; 64]));
        file.truncate(whole as usize + 20);
        assert_eq!(
            action_for(file),
            RecoveryAction::Remux { audio_tracks: 1, truncate_to: Some(whole) }
        );
    }

    #[test]
    fn an_already_faststarted_file_is_left() {
        let mut file = bx("ftyp", b"isom\0\0\0\0");
        let mut moov = bx("mvhd", &[0; 100]);
        moov.extend(trak(b"vide"));
        moov.extend(trak(b"soun"));
        file.extend(bx("moov", &moov));
        file.extend(bx("mdat", &[0; 500]));
        assert_eq!(action_for(file), RecoveryAction::Leave);
    }

    /// Killed before the first fragment was whole: a header and nothing a
    /// player can show.
    #[test]
    fn a_header_only_file_is_unplayable() {
        assert_eq!(action_for(fragmented_with(1, 0)), RecoveryAction::Unplayable);
        let mut file = fragmented_with(1, 1);
        file.truncate(file.len() - 10);
        assert_eq!(action_for(file), RecoveryAction::Unplayable);
    }

    #[test]
    fn a_file_killed_mid_moov_or_not_an_mp4_is_unplayable() {
        let mut file = fragmented_with(1, 0);
        file.truncate(file.len() - 10);
        assert_eq!(action_for(file), RecoveryAction::Unplayable);
        assert_eq!(action_for(b"partial but playable".to_vec()), RecoveryAction::Unplayable);
    }

    /// The same four decisions against files a real ffmpeg wrote.
    #[test]
    fn recovery_action_on_real_files() {
        let Some(ffmpeg) = fixtures::ffmpeg() else {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        };
        let dir = fixtures::dir("recovery-action");
        let finished = dir.join("finished.mp4");
        if fixtures::fragmented(&ffmpeg, &finished, 2).is_none() {
            return;
        }
        let boxes = fixtures::boxes(&finished);
        let summary = fixtures::summary(&finished);
        assert!(summary.mfra, "ffmpeg writes a fragment index on a clean finish");
        assert_eq!(
            recovery_action(&summary),
            RecoveryAction::Remux { audio_tracks: 2, truncate_to: None }
        );

        let (_, offset, size) = boxes.iter().filter(|b| b.0 == "mdat").nth(2).unwrap().clone();
        let killed = dir.join("killed.mp4");
        fixtures::truncated_copy(&finished, &killed, offset + size / 2);
        assert_eq!(
            recovery_action(&fixtures::summary(&killed)),
            RecoveryAction::Remux { audio_tracks: 2, truncate_to: None }
        );

        let header_only = dir.join("header-only.mp4");
        fixtures::truncated_copy(&finished, &header_only, summary.first_moof_offset.unwrap());
        assert_eq!(recovery_action(&fixtures::summary(&header_only)), RecoveryAction::Unplayable);

        let faststarted = dir.join("faststarted.mp4");
        std::fs::copy(&finished, &faststarted).unwrap();
        crate::recorder::remux::remux_faststart(&ffmpeg, &faststarted, 2).unwrap();
        assert_eq!(recovery_action(&fixtures::summary(&faststarted)), RecoveryAction::Leave);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// End to end: a daemon killed mid-game, then a restart. The row comes
    /// back finished, the file comes back moov-first with every track, and
    /// the markers are untouched.
    #[test]
    fn recovery_remuxes_a_killed_recording() {
        let Some(ffmpeg) = fixtures::ffmpeg() else {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        };
        let dir = fixtures::dir("recover-remux");
        let whole = dir.join("whole.mp4");
        if fixtures::fragmented(&ffmpeg, &whole, 2).is_none() {
            return;
        }
        let (_, offset, size) =
            fixtures::boxes(&whole).into_iter().filter(|b| b.0 == "mdat").nth(3).unwrap();
        let file = dir.join("recording-1.mp4");
        fixtures::truncated_copy(&whole, &file, offset + size / 2);
        // An old mtime, as a file left by a daemon that died last week has.
        let killed_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(killed_at).unwrap();

        let db = Db::open_temporary().unwrap();
        let id = db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();
        db.insert_markers(
            id,
            &[crate::db::NewMarker {
                game_time_s: 1.0,
                video_time_s: 1.0,
                kind: "kill".into(),
                payload_json: "{}".into(),
            }],
        )
        .unwrap();

        let report = recover_unfinished(&db, Some(&ffmpeg)).unwrap();
        assert_eq!(report.recovered, 1);

        let after = fixtures::summary(&file);
        assert!(!after.mvex, "remuxed out of fragments");
        assert!(after.moov_offset < after.first_mdat_offset, "the index is up front");
        assert_eq!(after.audio_tracks, 2, "every stem survived");
        assert!(!crate::recorder::remux::tmp_path(&file).exists());
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), killed_at);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].size_bytes, after.file_len as i64, "the size of the remuxed file");
        let duration = rows[0].duration_s.expect("probed after the remux");
        assert!(duration > 1.0 && duration < 3.0, "a partial recording: {duration}");
        assert_eq!(db.get_markers(id).unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The kill that lands in a `moof`. ffmpeg will not open that file at
    /// all, so without the cut the remux fails and the file stays unplayable.
    #[test]
    fn recovery_cuts_a_half_moof_and_remuxes() {
        let Some(ffmpeg) = fixtures::ffmpeg() else {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        };
        let dir = fixtures::dir("recover-moof");
        let whole = dir.join("whole.mp4");
        if fixtures::fragmented(&ffmpeg, &whole, 1).is_none() {
            return;
        }
        let (_, offset, size) =
            fixtures::boxes(&whole).into_iter().filter(|b| b.0 == "moof").nth(3).unwrap();
        let file = dir.join("recording-2.mp4");
        fixtures::truncated_copy(&whole, &file, offset + size * 2 / 3);

        let db = Db::open_temporary().unwrap();
        db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();
        assert_eq!(recover_unfinished(&db, Some(&ffmpeg)).unwrap().recovered, 1);

        let after = fixtures::summary(&file);
        assert!(!after.mvex && after.truncated.is_none(), "remuxed: {}", after.layout);
        assert!(db.list_recordings().unwrap()[0].duration_s.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// No ffmpeg: nothing to remux with, and the row is finished anyway.
    #[test]
    fn recovery_without_an_ffmpeg_still_finishes_the_row() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-no-ffmpeg");
        let file = dir.join("recording-3.mp4");
        let mut bytes = fragmented_with(1, 2);
        bytes.truncate(bytes.len() - 100);
        std::fs::write(&file, &bytes).unwrap();
        db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();

        assert_eq!(recover_unfinished(&db, None).unwrap().recovered, 1);
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "left as the kill left it");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A crash mid-remux leaves the temp file beside the recording. It is a
    /// half-written copy, never a recording, and must not be imported as one,
    /// in the current name or the `.mp4` one builds before #233 used.
    #[test]
    fn a_leftover_remux_temp_file_is_not_imported() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("remux-tmp");
        let recording = dir.join("game.mp4");
        std::fs::write(crate::recorder::remux::tmp_path(&recording), b"half a remux").unwrap();
        std::fs::write(dir.join("older.faststart.tmp.mp4"), b"half a remux").unwrap();
        std::fs::write(dir.join("OLDER2.FASTSTART.TMP.MP4"), b"half a remux").unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recovery_removes_a_stale_remux_temp_file() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-stale-tmp");
        let file = dir.join("recording-4.mp4");
        std::fs::write(&file, b"partial").unwrap();
        let tmp = crate::recorder::remux::tmp_path(&file);
        std::fs::write(&tmp, b"half a remux").unwrap();
        db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();

        assert_eq!(recover_unfinished(&db, None).unwrap().recovered, 1);
        assert!(!tmp.exists());
        assert!(file.exists(), "the recording itself is kept");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_recordings_dir_is_not_an_error() {
        let db = Db::open_temporary().unwrap();
        let dir = std::env::temp_dir().join("ninja-recorder-reconcile-does-not-exist");
        std::fs::remove_dir_all(&dir).ok();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report, ReconcileReport::default());
    }
}
