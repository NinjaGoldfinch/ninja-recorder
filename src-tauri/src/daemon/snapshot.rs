//! Full-state snapshot, for `hello` and for resync after a dropped pipe.
//!
//! A client that connects, or reconnects, gets one snapshot and then a stream
//! of events; it never replays history. That is what makes a UI killed
//! mid-game correct the moment it comes back (implementation plan §4.2, §4.3).
//!
//! ## Why this is not just `Snapshot::assemble`
//!
//! `assemble` reads everything a `Ctx` owns, and then asks for the two things
//! it does not: the position in the event stream, and the last observed LCU
//! status. Neither belongs to `Ctx` — the first is a property of the broadcast
//! and the second of a watcher that reports by event rather than by being
//! polled — so something has to hold them. That something is `Stream`, which
//! sits on the one path every event already takes and reads them off it.
//!
//! Putting it on the publish path rather than beside it is the point: an event
//! that reached a subscriber but not the counter would make `seq` a number
//! that means nothing, and a snapshot is the only thing that can tell a
//! reconnecting client where the stream it is about to read begins.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex as AsyncMutex;

use crate::contract::events::Event;
use crate::contract::snapshot::Snapshot;
use crate::core::{Ctx, LcuStatus};
use crate::state_machine::machine::GameState;
use crate::daemon::rpc::{Events, SnapshotSource};

/// The daemon's event stream: the broadcast, plus what having published to it
/// implies about the daemon's state.
///
/// Cloneable and cheap, because every producer needs one — the supervisor's
/// sink, the trim and summary callbacks, and the shutdown path.
#[derive(Clone)]
pub struct Stream {
    events: Events,
    seen: Arc<Seen>,
}

/// What the stream has gone past.
struct Seen {
    /// How many events have been published. `Snapshot::seq` is this value at
    /// the moment the snapshot was taken, so a client knows every event at or
    /// below it is already folded into what it just received.
    ///
    /// Relaxed ordering throughout: this is a monotone counter read by a
    /// snapshot that is a consistent-enough view by design, not a lock, and
    /// nothing branches on it.
    seq: AtomicU64,
    /// The last thing `Event::LcuPhase` said.
    ///
    /// A `std::sync::Mutex` would be simpler, and would be a lock held across
    /// nothing — but `publish` is called from inside the supervisor, which
    /// runs on both blocking and async threads, and `assemble` is called from
    /// a task. The async mutex keeps that honest rather than relying on every
    /// future caller noticing.
    lcu: AsyncMutex<LcuStatus>,
}

impl Stream {
    pub fn new(events: Events) -> Self {
        Self {
            events,
            seen: Arc::new(Seen {
                seq: AtomicU64::new(0),
                // Not `connected: false` as a guess but as the truth at
                // startup: nothing has seen a client yet, and the lockfile
                // watch says so within two seconds if one is there.
                lcu: AsyncMutex::new(LcuStatus {
                    connected: false,
                    phase: None,
                    summoner: None,
                    error: None,
                }),
            }),
        }
    }

    /// The one way an event reaches a subscriber.
    ///
    /// Counting first and publishing second, so a client that receives an
    /// event and then asks for a snapshot cannot be told the stream is behind
    /// where it has already read to.
    pub fn publish(&self, event: Event) {
        self.seen.seq.fetch_add(1, Ordering::Relaxed);
        self.observe(&event);
        self.events.publish(event);
    }

    /// Folds an event into the state a snapshot reports.
    ///
    /// ## `connected` comes from the state machine, not from `client_present`
    ///
    /// The two are not the same claim, and reading them as one puts a lie in
    /// every snapshot taken during a finalize. `LcuPhase { client_present:
    /// false }` is published by `stop_gameflow_watch`, which runs at
    /// end-of-game while League is still open and the lockfile is deliberately
    /// still held — it means "nothing is watching the phase right now", not
    /// "the client is gone". The state machine is the thing that actually
    /// knows: `GameState::Idle` is defined as the client having gone away, and
    /// every other state implies a lockfile.
    ///
    /// So `client_present: true` is taken at face value, because a phase can
    /// only have come from a client that answered, and `false` clears the
    /// phase without touching `connected`.
    fn observe(&self, event: &Event) {
        // `try_lock` rather than `blocking_lock`: this runs on the
        // supervisor's thread, sometimes inside a finalize holding the recorder
        // lock, and blocking there to record a status line would be a deadlock
        // risk taken for a cosmetic field. Contention is a snapshot being
        // assembled at the same instant, and the next event is a second away.
        let Ok(mut lcu) = self.seen.lcu.try_lock() else {
            return;
        };
        match event {
            Event::LcuPhase { phase, client_present } => {
                // Formatted exactly as `core::lcu_status` formats it, because
                // the two fill the same field and a client comparing them
                // should not find two spellings of one phase.
                lcu.phase = phase.as_ref().map(|p| format!("{p:?}"));
                if *client_present {
                    lcu.connected = true;
                }
                lcu.error = None;
            }
            Event::StateChanged { state, .. } => {
                lcu.connected = !matches!(state, GameState::Idle);
                if !lcu.connected {
                    // No client, so no phase. Keeping the last one would have a
                    // UI showing "In Progress" for a game that ended when
                    // League did.
                    lcu.phase = None;
                }
            }
            _ => {}
        }
    }

    /// A handle on the broadcast, for `serve` to subscribe with.
    pub fn events(&self) -> Events {
        self.events.clone()
    }

    /// The closure `hello` answers with.
    ///
    /// ## What it cannot know
    ///
    /// `LcuStatus::summoner` stays `None` here. The name is not in
    /// `Event::LcuPhase` and the only thing that knows it is a request to the
    /// League client, which is async and can fail — neither of which a
    /// snapshot source may be. A client that wants the name calls
    /// `lcu_status`, which is the command that exists to ask.
    pub fn source(&self, ctx: Arc<Ctx>) -> SnapshotSource {
        let seen = Arc::clone(&self.seen);
        Arc::new(move || {
            let seq = seen.seq.load(Ordering::Relaxed);
            // The same trade as `publish`, the other way round: a snapshot is
            // a consistent-enough view rather than an atomic one, so rather
            // than block a handshake on a lock the supervisor happens to hold,
            // report the client as absent — which is what a client that has
            // never been seen looks like, and which the next phase change
            // corrects within the second.
            let lcu = match seen.lcu.try_lock() {
                Ok(lcu) => lcu.clone(),
                Err(_) => LcuStatus {
                    connected: false,
                    phase: None,
                    summoner: None,
                    error: None,
                },
            };
            Snapshot::assemble(&ctx, seq, lcu)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::gameflow::GameflowPhase;

    fn stream() -> Stream {
        Stream::new(Events::new())
    }

    #[tokio::test]
    async fn seq_counts_every_event_that_was_published() {
        let stream = stream();
        // No subscriber, which is the ordinary state of a daemon with no UI
        // attached — and must not be the reason the counter stands still.
        for _ in 0..3 {
            stream.publish(Event::Lagged { dropped: 0 });
        }
        assert_eq!(stream.seen.seq.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn the_lcu_status_follows_the_phase_events() {
        let stream = stream();
        stream.publish(Event::LcuPhase {
            phase: Some(GameflowPhase::InProgress),
            client_present: true,
        });

        let lcu = stream.seen.lcu.lock().await.clone();
        assert!(lcu.connected);
        // The same spelling `core::lcu_status` produces, which is the whole
        // reason this is a `Debug` format and not a hand-written string.
        assert_eq!(lcu.phase.as_deref(), Some("InProgress"));
    }

    /// The state machine is what says the client has gone, and `Idle` is the
    /// state that means it. A snapshot that kept the last phase afterwards
    /// would have a UI showing "In Progress" for a game that ended when League
    /// did.
    #[tokio::test]
    async fn an_idle_state_machine_means_the_client_is_gone() {
        let stream = stream();
        stream.publish(Event::LcuPhase {
            phase: Some(GameflowPhase::InProgress),
            client_present: true,
        });
        stream.publish(Event::StateChanged { state: GameState::Idle, since_ms: 0 });

        let lcu = stream.seen.lcu.lock().await.clone();
        assert!(!lcu.connected);
        assert_eq!(lcu.phase, None);
    }

    /// The case that made `connected` the state machine's answer rather than
    /// the event's. `stop_gameflow_watch` publishes `client_present: false` at
    /// end-of-game, while League is still open and the lockfile is still held,
    /// so believing it would report the client gone every time a game ended.
    #[tokio::test]
    async fn the_gameflow_watch_stopping_is_not_the_client_leaving() {
        let stream = stream();
        stream.publish(Event::StateChanged {
            state: GameState::ClientRunning,
            since_ms: 0,
        });
        stream.publish(Event::LcuPhase {
            phase: Some(GameflowPhase::InProgress),
            client_present: true,
        });

        // Exactly what a finalize publishes.
        stream.publish(Event::LcuPhase { phase: None, client_present: false });

        let lcu = stream.seen.lcu.lock().await.clone();
        assert!(lcu.connected, "the client is still running through a finalize");
        assert_eq!(lcu.phase, None, "but nothing is watching its phase");
    }

    /// The other half: a client the state machine has seen is reported as
    /// there, even before any phase has arrived. This is what a snapshot taken
    /// seconds after startup looks like, and it used to say `connected: false`
    /// beside a state of `ClientRunning`.
    #[tokio::test]
    async fn a_running_client_is_reported_before_its_first_phase() {
        let stream = stream();
        stream.publish(Event::StateChanged {
            state: GameState::ClientRunning,
            since_ms: 0,
        });

        let lcu = stream.seen.lcu.lock().await.clone();
        assert!(lcu.connected);
        assert_eq!(lcu.phase, None);
    }
}
