//! Writer and reader connections. WS6 tasks 6.1 to 6.3.
//!
//! v1 holds a single `Mutex<Connection>`, which is correct for one process and
//! wrong for two: the UI's list query would block the daemon's marker write.
//! `Pool` splits it into one writer and a `Vec` of readers.
//!
//! The schema does not change. WS6 is additive: WAL and `busy_timeout` at open,
//! then this split. Migrations stay append-only, because shipped builds have
//! already run the old ones (implementation plan §4.4).
//!
//! ## Why one writer and several readers
//!
//! SQLite allows exactly one writer at a time whatever we do, so a second write
//! connection would buy nothing and would turn a `Mutex` wait into an
//! `SQLITE_BUSY` we have to handle. Keeping the writer behind a mutex means
//! writes queue in the process, where waiting is free and ordered, rather than
//! at the database, where it is an error code.
//!
//! Reads are the opposite: WAL lets them run concurrently with the writer and
//! with each other, so the only thing stopping them is our own lock. Several
//! reader connections is what turns that lock from a queue into a fast path.
//!
//! ## Why the readers are `query_only`
//!
//! A read path that tries to write should fail loudly at the connection rather
//! than quietly racing the writer. `query_only = ON` makes SQLite refuse the
//! statement, so a method filed on the wrong side of the split is a test
//! failure rather than a rare interleaving nobody can reproduce.
//!
//! It is also what the UI process gets when WS3 splits it out: the daemon owns
//! every write, and the UI reads the same file directly rather than asking for
//! rows over the pipe.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use super::DbError;

/// How many reader connections to open.
///
/// Four rather than one-per-caller: the readers are the library grid, the
/// review timeline, the stats bar and whatever the dev portal is doing, and
/// those are the things that might overlap. Each connection is a file handle
/// and a page cache, so this is not free, and there is no evidence yet that
/// more would help. Revisit with a measurement, not a guess.
const READERS: usize = 4;

/// How long SQLite waits for a lock before returning `SQLITE_BUSY`.
///
/// Five seconds is far longer than any statement this app runs, which is the
/// point: under WAL the only thing a reader waits for is a checkpoint, and the
/// only thing the writer waits for is another writer that cannot exist in this
/// process. So the timeout is not a tuning parameter, it is the margin before
/// something genuinely wrong becomes a visible error.
const BUSY_TIMEOUT_MS: u32 = 5_000;

pub struct Pool {
    /// Every statement that changes the database. Behind a mutex so writes
    /// queue in-process rather than racing for the database lock.
    ///
    /// `Option` only so `Drop` can close it before deleting a temporary
    /// database; it is `Some` for the whole of a pool's useful life.
    writer: Option<Mutex<Connection>>,
    /// `query_only` connections. Never empty.
    readers: Vec<Mutex<Connection>>,
    /// Round-robin cursor. `Relaxed` because handing two callers the same
    /// reader is a slower read, not a wrong one.
    next: AtomicUsize,
    /// A directory to remove when this pool goes away. Only ever set by
    /// `open_temporary`.
    temp_dir: Option<std::path::PathBuf>,
}

impl Pool {
    /// Opens the writer, runs `init` on it, then opens the readers.
    ///
    /// In that order on purpose: the readers must not see a database whose
    /// migrations have not run, and `query_only` would stop them running any.
    pub fn open<F>(path: &Path, init: F) -> Result<Self, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<(), DbError>,
    {
        let mut writer = Connection::open(path)?;
        configure_writer(&writer)?;
        init(&mut writer)?;

        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let reader = Connection::open(path)?;
            configure_reader(&reader)?;
            readers.push(Mutex::new(reader));
        }
        Ok(Self {
            writer: Some(Mutex::new(writer)),
            readers,
            next: AtomicUsize::new(0),
            temp_dir: None,
        })
    }

    /// A pool that cannot write, for a process that must not. WS3.4, §4.4.
    ///
    /// The UI reads the library directly rather than pulling a thousand rows
    /// over the pipe, and every write belongs to the daemon (§3.1). Until now
    /// that was a convention held up by care: the UI opened a full pool and
    /// simply never used the writer. This makes it a property of the
    /// connections, so a write path that appears in the wrong process fails at
    /// SQLite with `attempt to write a readonly database` rather than racing
    /// the daemon.
    ///
    /// ## Every connection, including the one `write()` hands out
    ///
    /// Rather than leaving the writer `None` and panicking when something asks
    /// for it. A panic would be a crash in a shipped UI for what is a
    /// programming error, and it would fire before SQLite ever saw the
    /// statement. A `query_only` writer refuses the *statement*, which is the
    /// same way a reader on the wrong side of the split already fails, and it
    /// leaves the caller with an error to report rather than a dead process.
    ///
    /// ## No migrations, deliberately
    ///
    /// `init` is not run and cannot be: migrations write. The daemon owns them
    /// and has already run them by the time anything here reads, or is about
    /// to. Opening before that yields connections that see no tables yet, and
    /// SQLite hands the tables to an open connection as soon as another one
    /// creates them, so this recovers on its own rather than needing a retry.
    pub fn open_read_only(path: &Path) -> Result<Self, DbError> {
        let writer = Connection::open(path)?;
        configure_reader(&writer)?;

        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let reader = Connection::open(path)?;
            configure_reader(&reader)?;
            readers.push(Mutex::new(reader));
        }
        Ok(Self {
            writer: Some(Mutex::new(writer)),
            readers,
            next: AtomicUsize::new(0),
            temp_dir: None,
        })
    }

    /// A pool on a throwaway database file, for tests.
    ///
    /// **A file, not `:memory:`, and the name says so.** A plain
    /// `Connection::open_in_memory` gives each connection its *own* empty
    /// database, so the readers in a pool of them would see no tables at all;
    /// the `cache=shared` URI that is supposed to fix that is deprecated and
    /// not reliably available in the bundled SQLite. A file is what the app
    /// actually runs on, it is what makes WAL real rather than a no-op, and it
    /// is the only way the concurrency test means anything.
    ///
    /// The directory is removed when the pool drops. Following `log.rs`, which
    /// hand-rolls the same thing, rather than taking a dependency for it.
    #[cfg(test)]
    pub fn open_temporary<F>(init: F) -> Result<Self, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<(), DbError>,
    {
        static NEXT_DB: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-test-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| {
            DbError::Sqlite(rusqlite::Error::InvalidPath(
                format!("{}: {e}", dir.display()).into(),
            ))
        })?;
        let mut me = Self::open(&dir.join("library.sqlite"), init)?;
        me.temp_dir = Some(dir);
        Ok(me)
    }

    /// The write connection. Blocks until whatever else is writing is done.
    ///
    /// Panics on a poisoned lock, which matches every other path in this
    /// module: a panic mid-write means the database is in a state nothing here
    /// knows how to reason about.
    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.writer.as_ref().expect("the writer outlives every caller").lock().unwrap()
    }

    /// A read connection, round-robin.
    ///
    /// Blocks only on the one it picked, so `READERS` concurrent reads proceed
    /// together and the `READERS + 1`th waits for the shortest of them rather
    /// than for all of them.
    pub fn read(&self) -> MutexGuard<'_, Connection> {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[i].lock().unwrap()
    }
}

/// WAL, and the settings that only mean anything alongside it.
fn configure_writer(conn: &Connection) -> Result<(), DbError> {
    // WAL is the whole reason this split is worth anything: under the default
    // rollback journal a writer blocks every reader for the length of its
    // transaction, so four reader connections would queue exactly as one did.
    //
    // It is a property of the *database file*, not the connection, and it
    // persists once set, so setting it here on the writer sets it for every
    // reader that follows. `pragma_update` rather than `execute`, because
    // SQLite answers this one with a row and `execute` treats that as an error.
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // `FULL` fsyncs on every commit, which under WAL buys durability against
    // power loss at a cost paid 1 Hz while recording. `NORMAL` loses at most
    // the last commits on a power cut and is the documented pairing with WAL.
    // What it cannot lose is the *recording*: the file on disk is the source of
    // truth and `reconcile` rebuilds a missing row from it.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// A connection that cannot write, and says so at the statement.
fn configure_reader(conn: &Connection) -> Result<(), DbError> {
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Last, because it refuses the pragmas above once it is on.
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(())
}

impl Drop for Pool {
    /// Closes every connection before removing a temporary database.
    ///
    /// Order matters on Windows, where an open handle stops the file being
    /// deleted. `Drop::drop` runs before the fields are dropped, so the
    /// connections are closed here by hand rather than left to field order.
    fn drop(&mut self) {
        let Some(dir) = self.temp_dir.take() else { return };
        self.readers.clear();
        drop(self.writer.take());
        // Best effort: a leftover directory in the OS temp dir is untidy, not
        // a failing test, and panicking in a destructor would mask the real
        // assertion that got us here.
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    fn schema(conn: &mut Connection) -> Result<(), DbError> {
        conn.execute("CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, v INTEGER)", [])?;
        Ok(())
    }

    /// WS6.1's exit criterion, as far as it can be checked off Windows: the
    /// file is in WAL mode, which is what `library.sqlite-wal` appearing beside
    /// it means.
    #[test]
    fn the_database_is_in_wal_mode() {
        let pool = Pool::open_temporary(schema).unwrap();
        let mode: String = pool
            .write()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "WAL is what makes the reader split worth anything");

        // Readers inherit it, because it is a property of the file.
        let mode: String = pool
            .read()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    /// WS6.3. A read path that tries to write fails at the connection rather
    /// than quietly racing the writer.
    #[test]
    fn a_write_on_a_reader_is_refused() {
        let pool = Pool::open_temporary(schema).unwrap();
        let err = pool
            .read()
            .execute("INSERT INTO t (v) VALUES (1)", [])
            .expect_err("a query_only connection must refuse a write");
        assert!(
            err.to_string().to_lowercase().contains("readonly"),
            "the refusal should name the reason, got: {err}"
        );
        // And the writer is unaffected by the refusal.
        pool.write().execute("INSERT INTO t (v) VALUES (1)", []).unwrap();
    }

    /// WS6.2's exit criterion: a long read does not block a write.
    ///
    /// The read is held open for the whole measurement, on its own connection,
    /// while the writer commits. Under the old single `Mutex<Connection>` the
    /// write could not even start until the read let go; the assertion is that
    /// it now finishes while the read is still running.
    #[test]
    fn a_long_read_does_not_block_a_write() {
        let pool = Arc::new(Pool::open_temporary(schema).unwrap());
        let reading = Arc::new(AtomicBool::new(true));

        let held = {
            let pool = Arc::clone(&pool);
            let reading = Arc::clone(&reading);
            std::thread::spawn(move || {
                let conn = pool.read();
                let mut stmt = conn.prepare("SELECT id, v FROM t").unwrap();
                // An open statement is a held read transaction, which is the
                // thing that used to block writers.
                let _rows = stmt.query_map([], |_| Ok(())).unwrap().count();
                while reading.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };

        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        pool.write().execute("INSERT INTO t (v) VALUES (42)", []).unwrap();
        let took = started.elapsed();

        reading.store(false, Ordering::Relaxed);
        held.join().unwrap();

        assert!(
            took < Duration::from_secs(1),
            "the write waited {took:?} for a reader, which is the problem the pool exists to fix"
        );
    }

    /// WS6.4's exit criterion: zero `SQLITE_BUSY`.
    ///
    /// The shape of the real thing, compressed. The supervisor writes markers
    /// at 1 Hz for the length of a game while the library grid lists rows; this
    /// runs both far faster than reality for two seconds, so a lock problem
    /// that would take a 35-minute game to show up has many more chances to
    /// appear here than it would in one.
    ///
    /// Not 60 seconds: this runs on every `cargo test`, and a minute of wall
    /// clock in the gate would be paid by every change forever. The failure it
    /// is looking for is contention, which is a function of overlap rather than
    /// of duration.
    #[test]
    fn writes_and_reads_do_not_collide() {
        let pool = Arc::new(Pool::open_temporary(schema).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicUsize::new(0));

        let writer = {
            let (pool, stop, busy) = (Arc::clone(&pool), Arc::clone(&stop), Arc::clone(&busy));
            std::thread::spawn(move || {
                let mut n = 0;
                while !stop.load(Ordering::Relaxed) {
                    if let Err(e) = pool.write().execute("INSERT INTO t (v) VALUES (?1)", [n]) {
                        if e.to_string().to_lowercase().contains("busy") {
                            busy.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    n += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
                n
            })
        };

        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (pool, stop, busy) = (Arc::clone(&pool), Arc::clone(&stop), Arc::clone(&busy));
                std::thread::spawn(move || {
                    let mut n = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let conn = pool.read();
                        match conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0)) {
                            Ok(_) => n += 1,
                            Err(e) => {
                                if e.to_string().to_lowercase().contains("busy") {
                                    busy.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    n
                })
            })
            .collect();

        std::thread::sleep(Duration::from_secs(2));
        stop.store(true, Ordering::Relaxed);

        let written = writer.join().unwrap();
        let read: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();

        assert_eq!(busy.load(Ordering::Relaxed), 0, "SQLITE_BUSY is the thing WS6 exists to remove");
        // Guards against the test passing because nothing actually ran.
        assert!(written > 10, "the writer only managed {written} inserts");
        assert!(read > 100, "the readers only managed {read} queries");
    }

    // --- the read-only pool -------------------------------------------------

    /// A directory no other test, and no earlier run, will use.
    ///
    /// The process id and a counter are not enough between them, which cost a
    /// CI run to establish. The counter restarts at 0 in every test binary and
    /// CI runs `cargo test` twice in one job, once with `--features devtools`;
    /// Windows reuses process ids freely inside a job; and the cleanup at the
    /// end of each test below used to run while the pools were still open, so
    /// on Windows the delete failed silently against a live file handle. The
    /// second run then opened the first run's migrated database, and
    /// `opening_before_the_schema_exists_recovers_when_it_appears` found a
    /// schema where it asserts there is none.
    ///
    /// The clock separates runs. Removing any survivor first makes a collision
    /// harmless rather than merely unlikely, which matters because the failure
    /// it caused was intermittent and looked like flakiness.
    fn scratch_dir(prefix: &str) -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-{prefix}-{}-{nanos:x}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A pool on a throwaway file, opened the way the UI opens the library.
    ///
    /// Two pools over one file, which is the arrangement being tested: the
    /// daemon's writer made the schema, and the UI's connections can only read
    /// it.
    fn two_process_pools() -> (Pool, Pool, std::path::PathBuf) {
        let dir = scratch_dir("ro");
        let path = dir.join("library.sqlite");

        let daemon = Pool::open(&path, schema).unwrap();
        let ui = Pool::open_read_only(&path).unwrap();
        (daemon, ui, dir)
    }

    /// The point of the whole change: the UI's *writer* refuses too.
    ///
    /// It is handed out rather than withheld so that a write path appearing in
    /// the wrong process is an error the caller can report, not a panic that
    /// takes the window down. What must not happen is the write succeeding.
    #[test]
    fn a_read_only_pool_refuses_a_write_on_every_connection() {
        let (_daemon, ui, dir) = two_process_pools();

        let error = ui
            .write()
            .execute("INSERT INTO t (v) VALUES (1)", [])
            .expect_err("the UI's writer must refuse a write");
        assert!(
            error.to_string().contains("readonly"),
            "SQLite should be what refuses it, got: {error}"
        );

        let error = ui
            .read()
            .execute("INSERT INTO t (v) VALUES (1)", [])
            .expect_err("a reader must refuse a write");
        assert!(error.to_string().contains("readonly"), "got: {error}");

        // Before the delete: Windows refuses to remove a file something still
        // holds open, and `remove_dir_all`'s failure here is ignored.
        drop((_daemon, ui));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// And it still *reads*, including rows the other process wrote after it
    /// opened. That is the half that makes the UI's library grid work without
    /// pulling a thousand rows over the pipe.
    #[test]
    fn a_read_only_pool_sees_what_the_writer_commits() {
        let (daemon, ui, dir) = two_process_pools();

        daemon.write().execute("INSERT INTO t (v) VALUES (42)", []).unwrap();

        let value: i64 =
            ui.read().query_row("SELECT v FROM t LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(value, 42, "the UI reads the daemon's writes from the same file");

        drop((daemon, ui));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The UI can open the library before the daemon has created it, because
    /// the UI is what starts the daemon. The tables appear on the connection
    /// that was already open, without it being reopened.
    #[test]
    fn opening_before_the_schema_exists_recovers_when_it_appears() {
        let dir = scratch_dir("ro-early");
        let path = dir.join("library.sqlite");

        // The UI first, on a path with nothing behind it.
        let ui = Pool::open_read_only(&path).unwrap();
        assert!(
            ui.read().query_row("SELECT v FROM t LIMIT 1", [], |r| r.get::<_, i64>(0)).is_err(),
            "there is no schema yet"
        );

        // Then the daemon, which migrates.
        let daemon = Pool::open(&path, schema).unwrap();
        daemon.write().execute("INSERT INTO t (v) VALUES (7)", []).unwrap();

        let value: i64 =
            ui.read().query_row("SELECT v FROM t LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(value, 7, "the same connection sees the table once it exists");

        drop((daemon, ui));
        let _ = std::fs::remove_dir_all(dir);
    }
}
