//! The raw League documents each recording's data is derived from (#349).
//!
//! **Why keep them at all.** Everything the app shows about a game (the
//! scoreboard, the runes, the trinket, the role, the stats to come) is
//! extracted from a handful of documents. Keeping only the extraction means a
//! change to it, or a new field, needs the League client again, and the
//! client's match history only reaches back so far. Keeping the documents
//! means an extraction can be re-run over them at any time, locally.
//!
//! Stored gzipped, as the JSON text was received: parsing and re-serialising
//! would keep only what a struct models, which is exactly the loss this
//! exists to avoid. The compression is pure and tested here; the table is
//! migration 15.

use super::{Db, DbError};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::{params, OptionalExtension};
use std::io::{Read, Write};

/// Which document. The spellings are the table's CHECK list;
/// `every_kind_round_trips_through_sqlite` fails if they drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    /// `/lol-match-history/v1/games/{id}`: all ten players, as the game ended.
    Match,
    /// `/lol-match-history/v1/game-timelines/{id}`: per-minute frames.
    Timeline,
    /// `/lol-end-of-game/v1/eog-stats-block`, when it was this game's.
    Eog,
    /// `/lol-summoner/v1/current-summoner`: who "us" was in the match
    /// document, which the document itself cannot say.
    Summoner,
    /// The last Live Client `allgamedata` poll that carried a player list.
    Live,
}

impl DocumentKind {
    pub const ALL: [DocumentKind; 5] = [Self::Match, Self::Timeline, Self::Eog, Self::Summoner, Self::Live];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Timeline => "timeline",
            Self::Eog => "eog",
            Self::Summoner => "summoner",
            Self::Live => "live",
        }
    }
}

/// Gzips a document's text. Default compression: these are a few tens of
/// kilobytes, written once, and the level barely matters at that size.
pub fn compress(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // Writing into a Vec cannot fail.
    encoder.write_all(text.as_bytes()).expect("gzip into memory");
    encoder.finish().expect("gzip into memory")
}

/// The text back. `None` for bytes that are not gzip, or not UTF-8, which a
/// row written by `put_document` never is: a damaged row reads as missing
/// rather than failing whatever asked for it.
pub fn decompress(bytes: &[u8]) -> Option<String> {
    let mut text = String::new();
    GzDecoder::new(bytes).read_to_string(&mut text).ok()?;
    Some(text)
}

impl Db {
    /// Archives a document for a recording, replacing one of the same kind.
    /// `false` if the recording does not exist, which a recording deleted
    /// while its document was being fetched can do.
    pub fn put_document(
        &self,
        recording_id: i64,
        kind: DocumentKind,
        text: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let body = compress(text);
        let conn = self.pool.write();
        let written = conn.execute(
            "INSERT INTO game_documents (recording_id, kind, fetched_at, body)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM recordings WHERE id = ?1)
             ON CONFLICT (recording_id, kind)
             DO UPDATE SET fetched_at = excluded.fetched_at, body = excluded.body",
            params![recording_id, kind.as_str(), now, body],
        )?;
        Ok(written > 0)
    }

    /// A recording's document of one kind, as the text it was received as.
    pub fn get_document(&self, recording_id: i64, kind: DocumentKind) -> Result<Option<String>, DbError> {
        let body: Option<Vec<u8>> = self
            .pool
            .read()
            .query_row(
                "SELECT body FROM game_documents WHERE recording_id = ?1 AND kind = ?2",
                params![recording_id, kind.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(body.as_deref().and_then(decompress))
    }

    /// Whether a recording has a document of one kind, without reading it.
    pub fn has_document(&self, recording_id: i64, kind: DocumentKind) -> Result<bool, DbError> {
        Ok(self.pool.read().query_row(
            "SELECT EXISTS (SELECT 1 FROM game_documents WHERE recording_id = ?1 AND kind = ?2)",
            params![recording_id, kind.as_str()],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(db: &Db) -> i64 {
        db.begin_recording("C:/vods/a.mp4", 1000, None).unwrap()
    }

    #[test]
    fn compression_round_trips_and_is_smaller() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/lcu/match-history-paired.json"
        ));
        let packed = compress(text);
        assert_eq!(decompress(&packed).as_deref(), Some(text));
        assert!(packed.len() * 4 < text.len(), "{} of {} bytes", packed.len(), text.len());
    }

    #[test]
    fn damaged_bytes_read_as_missing() {
        assert_eq!(decompress(b"not gzip"), None);
    }

    #[test]
    fn a_document_is_stored_replaced_and_read_back() {
        let db = Db::open_temporary().unwrap();
        let id = recording(&db);
        assert!(!db.has_document(id, DocumentKind::Match).unwrap());
        assert!(db.put_document(id, DocumentKind::Match, r#"{"gameId":1}"#, 10).unwrap());
        assert!(db.put_document(id, DocumentKind::Match, r#"{"gameId":2}"#, 20).unwrap());
        assert_eq!(db.get_document(id, DocumentKind::Match).unwrap().as_deref(), Some(r#"{"gameId":2}"#));
        assert!(db.has_document(id, DocumentKind::Match).unwrap());
        assert_eq!(db.get_document(id, DocumentKind::Live).unwrap(), None);
    }

    #[test]
    fn every_kind_round_trips_through_sqlite() {
        let db = Db::open_temporary().unwrap();
        let id = recording(&db);
        for kind in DocumentKind::ALL {
            assert!(db.put_document(id, kind, kind.as_str(), 0).unwrap(), "{kind:?}");
            assert_eq!(db.get_document(id, kind).unwrap().as_deref(), Some(kind.as_str()));
        }
    }

    /// A recording deleted while its document was in flight: nothing is
    /// written, and nothing fails.
    #[test]
    fn a_document_for_no_recording_is_not_written() {
        let db = Db::open_temporary().unwrap();
        assert!(!db.put_document(999, DocumentKind::Match, "{}", 0).unwrap());
    }

    /// The documents describe the recording, and go with it: retention,
    /// the user's Delete and reconcile all delete the row.
    #[test]
    fn documents_go_with_their_recording() {
        let db = Db::open_temporary().unwrap();
        let id = recording(&db);
        db.put_document(id, DocumentKind::Live, "{}", 0).unwrap();
        db.delete_recording(id).unwrap();
        assert!(!db.has_document(id, DocumentKind::Live).unwrap());
    }
}
