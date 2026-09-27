//! When the capture worker exists: the pure half of `OwnRecorder`'s
//! lifetime rules, so each one is a unit test rather than a Windows run.
//!
//! The rules (#241):
//!
//! - **`prepare` spawns it**, because the supervisor calls `prepare` when the
//!   League client opens, and that is §2.2's pre-warm. `start` spawns it too
//!   if it is not up, because nothing may depend on `prepare` having run.
//! - **`release` ends it**, because the supervisor calls `release` when the
//!   client closes. While a recording is in flight the release is remembered
//!   instead, and carried out once `stop` has finalized. A `prepare` or
//!   `start` in between (the client came back) forgets it.
//! - **A worker that has died is not respawned mid-recording.** The recording
//!   it was writing is recovered from the disk at `stop`, and the next
//!   `prepare` or `start` spawns a fresh one. Since #299 that `stop` comes at
//!   once: the supervisor hears of the death from the worker's reply thread,
//!   stops the recording, and the `prepare` and `start` of the second
//!   recording of the same game follow straight after.
//!
//! So the worker never runs without League, and never outlives the recording
//! that was in flight when League went away.

/// What the supervisor asked of the recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Prepare,
    Start,
    Stop,
    Release,
}

/// What `OwnRecorder` should do about the worker for a [`Call`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Spawn a worker, then send the call to it.
    SpawnAndSend,
    /// Send the call to the worker that is up.
    Send,
    /// Send the stop, then shut the worker down: a release was waiting on it.
    SendThenShutDown,
    /// Ask the worker to exit and wait for it.
    ShutDown,
    /// The worker that was recording has gone: recover the file from disk.
    Recover,
    /// No worker is needed for this.
    Nothing,
}

/// What the recorder knows about the worker and the recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lifetime {
    /// A worker process is up (spawned, handshaken, and not seen to die).
    pub worker: bool,
    /// A recording is in flight, from a successful `start` to its `stop`.
    pub recording: bool,
    /// The client closed during a recording: shut the worker down once
    /// `stop` has finalized it.
    pub release_pending: bool,
}

impl Lifetime {
    /// Decides what to do for `call`, and remembers or forgets a pending
    /// release. `worker` and `recording` are the caller's to update from what
    /// actually happened; this only reads them.
    pub fn decide(&mut self, call: Call) -> Action {
        match call {
            Call::Prepare | Call::Start => {
                self.release_pending = false;
                if call == Call::Prepare && self.recording {
                    // Warm already, or recovering at stop: nothing to do.
                    Action::Nothing
                } else if self.worker {
                    Action::Send
                } else {
                    Action::SpawnAndSend
                }
            }
            Call::Stop => match (self.recording, self.worker) {
                (false, _) => Action::Nothing,
                (true, false) => Action::Recover,
                (true, true) if self.release_pending => Action::SendThenShutDown,
                (true, true) => Action::Send,
            },
            Call::Release => {
                if self.recording {
                    self.release_pending = true;
                    Action::Nothing
                } else if self.worker {
                    Action::ShutDown
                } else {
                    Action::Nothing
                }
            }
        }
    }

    /// Whether a worker should be running at all right now. The invariant the
    /// rules above keep: only between a `prepare`/`start` and the `release`
    /// (or the `stop` a release was waiting on).
    pub fn wants_worker(&self) -> bool {
        self.recording || (self.worker && !self.release_pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Lifetime = Lifetime { worker: false, recording: false, release_pending: false };
    const WARM: Lifetime = Lifetime { worker: true, recording: false, release_pending: false };
    const RECORDING: Lifetime = Lifetime { worker: true, recording: true, release_pending: false };

    fn decide(mut state: Lifetime, call: Call) -> Action {
        state.decide(call)
    }

    #[test]
    fn the_client_opening_spawns_the_worker() {
        assert_eq!(decide(IDLE, Call::Prepare), Action::SpawnAndSend);
        assert_eq!(decide(WARM, Call::Prepare), Action::Send);
    }

    /// `start` must not depend on `prepare` having run (§2.2).
    #[test]
    fn a_start_with_no_worker_spawns_one() {
        assert_eq!(decide(IDLE, Call::Start), Action::SpawnAndSend);
        assert_eq!(decide(WARM, Call::Start), Action::Send);
    }

    #[test]
    fn the_client_closing_ends_the_worker() {
        let mut state = WARM;
        assert_eq!(state.decide(Call::Release), Action::ShutDown);
        assert!(!state.release_pending);
    }

    #[test]
    fn with_no_client_there_is_nothing_to_end() {
        let mut state = IDLE;
        assert_eq!(state.decide(Call::Release), Action::Nothing);
        assert!(!state.wants_worker());
    }

    /// The client closing mid-recording does not end the worker; the stop
    /// that follows finalizes and then ends it.
    #[test]
    fn a_release_mid_recording_waits_for_the_stop() {
        let mut state = RECORDING;
        assert_eq!(state.decide(Call::Release), Action::Nothing);
        assert!(state.release_pending);
        assert!(state.wants_worker(), "still recording");
        assert_eq!(state.decide(Call::Stop), Action::SendThenShutDown);
    }

    /// The client came back before the game ended: the worker stays.
    #[test]
    fn a_prepare_or_start_forgets_a_pending_release() {
        for call in [Call::Prepare, Call::Start] {
            let mut state = RECORDING;
            state.decide(Call::Release);
            state.decide(call);
            assert!(!state.release_pending, "{call:?}");
            assert_eq!(state.decide(Call::Stop), Action::Send, "{call:?}");
        }
    }

    #[test]
    fn a_prepare_while_recording_does_nothing() {
        assert_eq!(decide(RECORDING, Call::Prepare), Action::Nothing);
    }

    /// The worker died mid-recording: the file is recovered from disk, and
    /// nothing is respawned for the stop.
    #[test]
    fn a_stop_after_the_worker_died_recovers_the_file() {
        let mut state = Lifetime { worker: false, ..RECORDING };
        assert_eq!(state.decide(Call::Stop), Action::Recover);
        state.release_pending = true;
        assert_eq!(state.decide(Call::Stop), Action::Recover);
    }

    /// And the next prepare after that spawns a fresh one.
    #[test]
    fn the_next_prepare_after_a_crash_spawns_a_fresh_worker() {
        let mut state = Lifetime { worker: false, recording: false, release_pending: false };
        assert_eq!(state.decide(Call::Prepare), Action::SpawnAndSend);
    }

    #[test]
    fn a_stop_with_nothing_recording_needs_no_worker() {
        assert_eq!(decide(IDLE, Call::Stop), Action::Nothing);
        assert_eq!(decide(WARM, Call::Stop), Action::Nothing);
    }

    /// Walk a whole session: client opens, game, client closes mid-game,
    /// stop. The worker is wanted exactly from the prepare to the stop.
    #[test]
    fn a_session_wants_a_worker_only_while_league_does() {
        let mut state = IDLE;
        assert!(!state.wants_worker());
        assert_eq!(state.decide(Call::Prepare), Action::SpawnAndSend);
        state.worker = true;
        assert!(state.wants_worker());
        assert_eq!(state.decide(Call::Start), Action::Send);
        state.recording = true;
        assert_eq!(state.decide(Call::Release), Action::Nothing);
        assert!(state.wants_worker());
        assert_eq!(state.decide(Call::Stop), Action::SendThenShutDown);
        state.recording = false;
        state.worker = false;
        state.release_pending = false;
        assert!(!state.wants_worker());
    }
}
