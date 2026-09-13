//! Writer and reader connections.
//!
//! **Empty — WS6 (tasks 6.2–6.3).** v1 holds a single `Mutex<Connection>`,
//! which is correct for one process and wrong for two: the UI's list query
//! would block the daemon's marker write. `Pool` splits it into one writer,
//! owned by the daemon, and a `Vec` of readers.
//!
//! The reader connections open with `query_only = ON`, so a read path that
//! tries to write fails loudly at the connection rather than quietly racing
//! the writer. Task 6.3 tests exactly that.
//!
//! The schema does not change. WS6 is additive: WAL and `busy_timeout` at open
//! (task 6.1), then this split. Migrations stay append-only — shipped builds
//! have already run the old ones (implementation plan §4.4).
