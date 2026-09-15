//! UI-side helper: start the daemon if none is listening.
//!
//! **Empty — WS3 task 3.5.** The UI connects to the pipe; if nothing answers,
//! it spawns `ninja-recorder.exe --daemon` and retries with a bounded backoff.
//!
//! Half of the drift problem is already solved: `rpc::endpoint` names the
//! address and `rpc::connect` opens it, so both sides read one spelling of it
//! from one file. What is left for this module is the other half — the
//! argument list, the detached spawn, and the backoff around it — plus
//! flipping the autostart flag from `--hidden` to `--daemon`, which is the
//! same agreement written into the registry (implementation plan §3.2).
