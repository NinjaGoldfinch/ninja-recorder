//! Full-state snapshot, for `hello` and for resync after a dropped pipe.
//!
//! **Empty — WS3 task 3.4.** A client that connects, or reconnects, gets one
//! snapshot and then a stream of events; it never replays history. This is
//! what makes a UI that was killed mid-game correct the moment it comes back
//! (implementation plan §4.2, §4.3).
