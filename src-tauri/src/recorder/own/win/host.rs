//! The capture worker's [`Host`]: the session thread, driven by the worker's
//! pipe loop instead of by `OwnRecorder`.
//!
//! This is the half of `OwnRecorder` that used to sit on the daemon's side of
//! the channel, moved into the worker unchanged. The session thread
//! (`session.rs`) does not know it moved: it still receives `Command`s on a
//! channel and answers on the senders they carry. What changed is only who
//! holds the other end, which is now `worker::serve`'s loop, reading the
//! same requests off stdin.

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

use super::session::{self, Command};
use crate::recorder::audio::AudioLayout;
use crate::recorder::own::plan::CapturePlan;
use crate::recorder::own::status::Status;
use crate::recorder::own::worker::protocol::Started;
use crate::recorder::own::worker::serve::{Host, Stopped};
use crate::warn;

/// The session thread, started on the first request and ended by `release`.
#[derive(Default)]
pub struct SessionHost {
    session: Option<SessionThread>,
    /// The origin for a start that has been answered and not yet begun.
    origin: Option<Sender<i64>>,
}

struct SessionThread {
    commands: Sender<Command>,
    thread: JoinHandle<()>,
}

impl SessionHost {
    /// Sends `command` to the session thread, starting it if it is not
    /// running, and waits for the answer on the channel `make` was given.
    /// A session thread that has gone (it panicked) is started afresh on the
    /// next request.
    fn ask<T>(&mut self, make: impl FnOnce(Sender<T>) -> Command) -> Result<T, String> {
        if self.session.is_none() {
            let (commands, receiver) = channel();
            let thread = std::thread::Builder::new()
                .name("own-capture".to_string())
                .spawn(move || session::run(receiver))
                .map_err(|e| format!("could not start the capture thread: {e}"))?;
            self.session = Some(SessionThread { commands, thread });
        }
        let (reply, answer) = channel();
        let session = self.session.as_ref().expect("just started");
        if session.commands.send(make(reply)).is_err() {
            self.session = None;
            return Err("the capture thread has exited".to_string());
        }
        answer.recv().map_err(|_| {
            self.session = None;
            "the capture thread exited before answering".to_string()
        })
    }
}

impl Host for SessionHost {
    fn prepare(&mut self) -> Result<Status, String> {
        self.origin = None;
        self.ask(Command::Prepare)?
    }

    fn start(&mut self, path: PathBuf, plan: AudioLayout) -> Result<Started, String> {
        self.origin = None;
        // The plan arrived as the layout it describes (`protocol::Request`).
        let plan = CapturePlan { sources: plan.sources, tracks: plan.tracks };
        let (origin, receiver) = channel();
        let started = self.ask(|reply| Command::Start { path, plan, reply, origin: receiver })??;
        self.origin = Some(origin);
        let summary = Some(started.summary);
        let problems = started.problems;
        Ok(Started { status: started.status, audio: started.audio, summary, problems })
    }

    fn origin(&mut self, qpc_hns: i64) {
        if let Some(origin) = self.origin.take() {
            // A session that has gone will say so at the next request.
            let _ = origin.send(qpc_hns);
        }
    }

    fn stop(&mut self) -> Stopped {
        // A start whose origin never came is abandoned by the session when
        // this sender goes: it finds the channel closed.
        self.origin = None;
        match self.ask(Command::Stop) {
            Ok(stopped) => Stopped {
                result: stopped.result,
                summary: stopped.summary,
                problems: stopped.problems,
            },
            Err(e) => Stopped::failed(e),
        }
    }

    /// Ends the session thread, finalizing anything it is still recording,
    /// and waits for it: every COM object is released by the time this
    /// returns.
    fn release(&mut self) {
        self.origin = None;
        if let Some(session) = self.session.take() {
            let _ = session.commands.send(Command::Release);
            if session.thread.join().is_err() {
                warn!("recorder", "own backend: the capture thread panicked");
            }
        }
    }
}
