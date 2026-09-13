//! UI-side helper: start the daemon if none is listening.
//!
//! **Empty — WS3 task 3.5.** The UI connects to the pipe; if nothing answers,
//! it spawns `ninja-recorder.exe --daemon` and retries with a bounded backoff.
//! Lives in `daemon/` rather than `ui/` because the two sides have to agree on
//! the pipe name and the argument list, and one file is how they cannot drift
//! (implementation plan §3.2).
